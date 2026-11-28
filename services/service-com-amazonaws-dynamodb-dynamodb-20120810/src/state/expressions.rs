// SPDX-License-Identifier: CC0-1.0

//! The expression language `PutItem`, `UpdateItem`, `DeleteItem`, `Query` and `Scan`
//! share: how an expression member is tokenized and parsed, the document paths it
//! addresses, the conditions it evaluates and the updates it applies.
//!
//! Two rules here are after the parse rather than inside it: a reserved word written out
//! as an attribute name ([`ExprParser::check_no_reserved_name`]) and a pair of
//! parentheses that groups what is already grouped
//! ([`ExprParser::check_no_redundant_parentheses`]) are each refused only once the
//! expression has PARSED, because an expression the grammar cannot complete is the syntax
//! error it is.

use super::*;
use std::collections::HashSet;
mod key_paths;
pub use key_paths::*;
mod operands;
pub use operands::*;
mod request_shape;
mod legacy_conditions;
mod value_algebra;
mod updates;
pub use updates::*;
pub use value_algebra::*;
pub use legacy_conditions::*;
use request_shape::*;

/// The most any one expression member may carry: "Maximum length of any expression
/// string is 4 KB" ("Service, Account, and Table Quotas"), plus the ceilings on the
/// tokens and the nesting that 4 KB of text can express. All three bound the work one
/// request can ask the parsers below for, so neither the token vector nor the parser's
/// stack use is a function of what a caller sends.
pub const MAX_EXPRESSION_BYTES: usize = 4096;
pub const MAX_EXPRESSION_TOKENS: usize = 1024;
pub const MAX_EXPRESSION_DEPTH: usize = 16;

/// The most OPERATORS AND FUNCTIONS one expression may state: "the maximum number of
/// operators or functions allowed in an expression is 300" ("Service, Account, and Table
/// Quotas", the expression-parameter entry), which that entry states with the example that
/// the expression `a IN (b, c, d)` contains ONE operator — the `IN` — however many values
/// its list carries.
///
/// A quota of the EXPRESSION LANGUAGE rather than of one member, so it is held wherever an
/// expression is read ([`tokenize_expression_spanned`]): a filter, a condition, a key
/// condition and an update are each refused past it.
pub const MAX_EXPRESSION_OPERATORS: usize = 300;

/// The most actions one update may state, whichever member states them.
///
/// An `UpdateExpression` is already bounded by the token ceiling above — the shortest
/// action an expression can spell takes three tokens (a path, an operator and a value)
/// — and this is that same bound, named, so the legacy `AttributeUpdates` map is held
/// to it too: that member carries no modelled length, so without this the number of
/// actions one request can ask the service to index, apply and write would be the
/// number of entries a request body can carry. Reaching it is the ordinary
/// `ValidationException`.
pub const MAX_UPDATE_ACTIONS: usize = MAX_EXPRESSION_TOKENS / 3;

/// The most entries one LEGACY CONDITION MAP — `ScanFilter`, `QueryFilter`, `Expected`
/// — may state.
///
/// The expression member each of those was replaced by is already bounded by the
/// ceilings above, and a legacy map carries no modelled length; without this the number
/// of comparisons one request can ask the service to evaluate FOR EVERY ITEM IT READS
/// would be the number of entries a request body can carry. The bound is that same
/// token ceiling read as comparisons — the shortest comparison an expression can spell
/// takes three tokens, a name, an operator and a value — so one legacy map is held to
/// what one expression of the documented size could state. Reaching it is the ordinary
/// `ValidationException`.
pub const MAX_LEGACY_CONDITIONS: usize = MAX_EXPRESSION_TOKENS / 3;

/// The most values one `IN` OPERATOR of the expression language may list: "The list can
/// contain up to 100 values, separated by commas" (the Developer Guide's
/// comparison-operator reference for `IN`).
///
/// A ceiling of that OPERATOR, so it binds the expression spelling alone. The legacy
/// `Condition$AttributeValueList` the pre-expression filters compare through states no such
/// count — "AttributeValueList can contain one or more AttributeValue elements of type
/// String, Number, or Binary" (`ComparisonOperator`, the `IN` entry) — so a legacy entry is
/// served however many values it lists, held only to the ceiling every collection a request
/// carries is held to ([`MAX_COLLECTION_MEMBERS`]).
pub const MAX_IN_OPERANDS: usize = 100;

/// The most members one COLLECTION attribute — a set, a list or a map — may carry,
/// whether a request sent it or an update computed it. This is the EMULATOR's own
/// limit, not an AWS quota.
///
/// A collection's cardinality is what every operation over it costs (comparison, set
/// union and difference, a `list_append` concatenation), and it is the request's to
/// choose. The documented 400 KB item limit does not bound it on its own, because a
/// member's accounted size can be zero (an empty string member). This ceiling is the
/// item limit's own byte count, so it refuses only collections far larger than one a
/// 400 KB item could hold with a byte per member, and it keeps the work one request can
/// ask for a function of this service's constants.
pub const MAX_COLLECTION_MEMBERS: usize = 400 * 1024;

/// The message a conditional write's refusal carries.
///
/// The CONTRACT is the error code `ConditionalCheckFailedException` and its 400; this
/// is the text the service is observed to send with it, the same for every operation
/// and every unmet condition, so it reveals nothing about the stored item.
pub const CONDITION_FAILED_MESSAGE: &str = "The conditional request failed";

/// One comparison an expression makes between two operands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

impl CompareOp {
    /// The operator as an expression writes it, which is what a refusal that quotes
    /// the offending token has to state.
    fn as_str(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "<>",
            Self::Less => "<",
            Self::LessOrEqual => "<=",
            Self::Greater => ">",
            Self::GreaterOrEqual => ">=",
        }
    }

    /// Whether this comparison ORDERS its two operands rather than only asking whether
    /// they denote the same value.
    ///
    /// DynamoDB documents an order for the three scalar types alone, so only these
    /// comparisons hold their operands to it ([`check_orderable_operand`]); equality is
    /// defined between values of every type.
    fn orders(self) -> bool {
        !matches!(self, Self::Equal | Self::NotEqual)
    }
}

/// One lexical token of a DynamoDB expression.
///
/// The lexer is shared by the condition and update grammars below: both spell a
/// document path, an `#alias`, a `:placeholder` and a parenthesised argument list the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExprToken {
    /// A bare word: an attribute name, a section or operator keyword (`SET`, `AND`,
    /// ...) or a function name. Which it is, is the parser's business.
    Word(String),
    /// A `#alias` naming an `ExpressionAttributeNames` entry, with its `#`, which is
    /// how that map's keys are spelled.
    Alias(String),
    /// A `:placeholder` naming an `ExpressionAttributeValues` entry, with its `:`.
    Placeholder(String),
    /// A list index, as a document path writes it: `[3]`.
    Index(usize),
    Compare(CompareOp),
    Open,
    Close,
    Comma,
    Dot,
    Plus,
    Minus,
}

/// Which characters make up a bare word: an attribute name written out in an
/// expression, or a keyword. A name outside this set is reachable only through
/// `ExpressionAttributeNames`, which is what that member is for ("To prevent special
/// characters in an attribute name from being misinterpreted in an expression").
///
/// One alphabet for every expression grammar of this service, the
/// `KeyConditionExpression` one the `Query` handler reads included, so a name a request
/// has to alias is the same name in all of them.
pub fn is_expression_word_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_'
}

/// The refusal for an expression member past [`MAX_EXPRESSION_BYTES`]: the member it
/// was sent as and the rule it broke. One wording for one rule, whichever expression
/// member a request oversized.
///
/// Whether it carries the `<N> validation error(s) detected: ` envelope follows the
/// MEMBER, like every other refusal this service raises about a request's expressions
/// ([`envelopes_expression_refusals`]): the members a write states its intent through are
/// refused inside it, and the members a read narrows its result with are refused with the
/// sentence alone. The size ceiling is checked by this service's own expression code
/// rather than by a bound the model declares — the expression shapes carry no `@length`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ProjectionExpression")`,
/// and the same for `ConditionExpression`, `UpdateExpression` and `KeyExpression`, give a
/// plain string) — so it is worded by that one rule and not as a constraint clause.
pub fn expression_size_exceeded(member: &str) -> RequestError {
    expression_refusal(
        envelopes_expression_refusals(member),
        expression_clause(member, "Expression size has exceeded the maximum allowed size;"),
    )
}

/// The MEMBER a PartiQL statement's expressions are read as: the statement itself is what
/// the request carries, so it is the one "member" that is not an expression member of a
/// request at all ([`super::partiql`] renders a statement's clauses into this grammar and
/// parses them under this name).
pub(super) const STATEMENT_MEMBER: &str = "Statement";

/// One clause about an expression, stated for the member it travelled in.
///
/// Every refusal the expression languages raise names the member that carried the text —
/// `Invalid <member>: <clause>` — because that is what tells a caller which member of its
/// request holds the mistake. The PartiQL surface has no such member: the request carries
/// a `Statement`, whose clauses are lowered onto this grammar, so AWS states those
/// refusals as the bare clause and a client reading one gets the defect class without a
/// member name in front of it ([`STATEMENT_MEMBER`]).
fn expression_clause(member: &str, detail: &str) -> String {
    if member == STATEMENT_MEMBER {
        return detail.to_string();
    }
    format!("Invalid {member}: {detail}")
}

/// One clause of the validation envelope for a refusal the EXPRESSION language raises:
/// AWS states the detail behind the expression member it came from, so a caller reading
/// the message knows which member of the request carried the mistake — except on the
/// statement surface, whose refusals are stated bare ([`expression_clause`]).
///
/// Whether the clause is wrapped in the `<N> validation error(s) detected: ` envelope
/// follows the MEMBER: the members a WRITE states its intent through are validated
/// inside it, and the members a READ narrows its result with are not — the service
/// states those on their own.
fn invalid_expression_member(member: &str, detail: &str) -> RequestError {
    expression_refusal(
        envelopes_expression_refusals(member),
        expression_clause(member, detail),
    )
}

/// The expression members whose refusals carry the validation envelope: the two a
/// write states its intent through.
const ENVELOPED_EXPRESSION_MEMBERS: [&str; 2] = ["UpdateExpression", "ConditionExpression"];

/// Whether a refusal about the expressions of a request carrying `member` is stated
/// inside the `<N> validation error(s) detected: ` envelope.
///
/// The ONE place that question is answered, for every refusal this service raises about
/// a request's expressions — an unreadable expression, a token no table defines, a table
/// entry no expression references — so the shape of the sentence follows the member the
/// request states its intent through and not the check that happened to notice.
fn envelopes_expression_refusals(member: &str) -> bool {
    ENVELOPED_EXPRESSION_MEMBERS.contains(&member)
}

/// State one expression refusal, inside the envelope or on its own.
fn expression_refusal(enveloped: bool, clause: String) -> RequestError {
    if enveloped {
        refused(clause)
    } else {
        RequestError::Validation(clause)
    }
}

/// The clause for a `#alias` an expression dereferences and `ExpressionAttributeNames`
/// does not define. ONE wording for that fact, whichever expression member carried the
/// token and whichever reader resolved it.
fn undefined_name_clause(alias: &str) -> String {
    format!(
        "An expression attribute name used in the document path is not defined; \
         attribute name: {alias}"
    )
}

/// The clause for a `:placeholder` an expression dereferences and
/// `ExpressionAttributeValues` does not define.
fn undefined_value_clause(placeholder: &str) -> String {
    format!(
        "An expression attribute value used in expression is not defined; \
         attribute value: {placeholder}"
    )
}

/// The refusal for an undefined `#alias`, stated behind the expression member that
/// carried it — what every reader of an expression answers for a token the request never
/// defined.
pub fn undefined_name(member: &str, alias: &str) -> RequestError {
    invalid_expression_member(member, &undefined_name_clause(alias))
}

/// The refusal for an undefined `:placeholder`, stated behind the expression member that
/// carried it.
pub fn undefined_value(member: &str, placeholder: &str) -> RequestError {
    invalid_expression_member(member, &undefined_value_clause(placeholder))
}

/// The refusal for an expression member that carries no expression at all: AWS names the
/// member and states that fact, rather than pointing at a token there is none of.
///
/// One wording for every expression member of this service, whichever grammar reads it —
/// a member holding nothing (or nothing but whitespace) states no expression, which is a
/// different fact from one whose text a parser stopped inside.
pub fn expression_is_empty(member: &str) -> RequestError {
    invalid_expression_member(member, "The expression can not be empty;")
}

/// The refusal for a document path naming more levels than a stored attribute may nest
/// ([`MAX_ATTRIBUTE_DEPTH`]): AWS states the quota was broken and reports how deep the
/// offending path actually went, counted off the path the request wrote, rather than
/// restating the limit.
///
/// One wording for every expression member a path can be written in — a projection, a
/// filter, a condition, a key condition and an update all address the item through the same
/// path grammar — so the sentence cannot drift between them, and it carries the validation
/// envelope exactly where every other refusal about that member's expression does
/// ([`invalid_expression_member`]).
fn too_many_nesting_levels(member: &str, levels: usize) -> RequestError {
    invalid_expression_member(
        member,
        &format!("The document path has too many nesting levels; nesting levels: {levels}"),
    )
}

/// The refusal for an operand position a function reads an item LOCATION through, given
/// anything other than a document path: AWS names the function and that it takes a path.
///
/// The functions that read an attribute of the item — `attribute_exists`,
/// `attribute_type`, `begins_with`, `contains`, `size`, `if_not_exists` — take a PATH
/// first ("attribute_exists (path)" and the rest of the `ConditionExpression` function
/// reference spell it), so a `:placeholder` written there names no location at all. Worded
/// the same way the statement surface words that defect.
fn requires_document_path(member: &str, operator_or_function: &str) -> RequestError {
    invalid_expression_member(member, &document_path_required_clause(operator_or_function))
}

/// That defect's CLAUSE, stated once for every surface of this service that reads a call:
/// the expression members below state it behind the member that carried the expression, and
/// the statement surface states it on its own ([`super::partiql`]), so the sentence cannot
/// drift between the two.
pub(super) fn document_path_required_clause(operator_or_function: &str) -> String {
    format!(
        "Operator or function requires a document path; operator or function: \
         {operator_or_function}"
    )
}

/// The refusal for a call carrying a number of operands its function does not take: AWS
/// names the function and how many operands the call carried, both read off the request.
///
/// The arity is a fact of the call's own TEXT, so it is settled where the expression is
/// read rather than reported as the position the grammar stopped at.
fn incorrect_operand_count(
    member: &str,
    operator_or_function: &str,
    operands: usize,
) -> RequestError {
    invalid_expression_member(
        member,
        &operand_count_clause(operator_or_function, operands),
    )
}

/// That defect's CLAUSE, stated once for both surfaces, like the one above.
pub(super) fn operand_count_clause(operator_or_function: &str, operands: usize) -> String {
    format!(
        "Incorrect number of operands for operator or function; operator or function: \
         {operator_or_function}, number of operands: {operands}"
    )
}

/// One attribute value as a refusal about an OPERAND echoes it: its type tag and the text
/// it carries, in the shape this family of messages states an operand in.
/// Only the three types DynamoDB ORDERS can reach this — the two bounds of a `BETWEEN`
/// are held to those before they are compared ([`check_orderable_operand`]) — so a
/// collection carries no text of its own here.
fn rendered_operand(value: &AttributeValue) -> String {
    let text = match value {
        AttributeValue::S(text) | AttributeValue::N(text) => text.clone(),
        AttributeValue::B(bytes) => ::aws_smithy_types::base64::encode(bytes),
        _ => String::new(),
    };
    format!("AttributeValue: {{{}:{text}}}", attribute_value_type(value))
}

/// The refusal for a `BETWEEN` whose two bounds the request stated the wrong way round.
///
/// "BETWEEN — ... true if a is greater than or equal to b, and less than or equal to c"
/// (the `ConditionExpression` comparator reference): a lower bound above the upper one
/// selects nothing whatever the item holds, so it is the request's mistake and not an
/// unmet condition. AWS echoes both bounds as the operands they are.
fn between_bounds_out_of_order(
    member: &str,
    low: &AttributeValue,
    high: &AttributeValue,
) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "The BETWEEN operator requires upper bound to be greater than or equal to lower \
             bound; lower bound operand: {}, upper bound operand: {}",
            rendered_operand(low),
            rendered_operand(high)
        ),
    )
}

/// The refusal for a `BETWEEN` whose two bounds the request stated in DIFFERENT types.
///
/// "BETWEEN — ... true if a is greater than or equal to b, and less than or equal to c"
/// (the `ConditionExpression` comparator reference): the two bounds draw one range, and
/// DynamoDB orders values only WITHIN a type ("For type Number, value comparisons are
/// numeric. String value comparisons ... are based on ASCII character code values" — the
/// `AttributeValueList` doc), so bounds of two types draw no range at all. That is the
/// request's mistake and not an unmet condition, and AWS echoes both bounds as the
/// operands they are, exactly as the refusal for two bounds the wrong way round does
/// ([`between_bounds_out_of_order`]).
fn between_bounds_of_different_types(
    member: &str,
    low: &AttributeValue,
    high: &AttributeValue,
) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "The BETWEEN operator requires same data type for lower and upper bounds; lower \
             bound operand: {}, upper bound operand: {}",
            rendered_operand(low),
            rendered_operand(high)
        ),
    )
}

/// Hold one `BETWEEN`'s bounds to the comparator's two rules, where the request stated
/// both of them as LITERALS — a bound read from the item is known only once the item is
/// read.
///
/// The bounds must be of ONE type, because a range is drawn inside a type
/// ([`between_bounds_of_different_types`]), and the lower one must not sit above the
/// upper one ([`between_bounds_out_of_order`]). The type rule is asked first: two bounds
/// of different types are not in any order to begin with.
///
/// The one statement of both rules, for every member a `BETWEEN` can be written in: the
/// condition and filter grammars this module parses, and the `KeyConditionExpression`
/// grammar the `Query` handler parses.
pub fn check_between_bounds(
    member: &str,
    low: &AttributeValue,
    high: &AttributeValue,
) -> Result<(), RequestError> {
    if attribute_value_type(low) != attribute_value_type(high) {
        return Err(between_bounds_of_different_types(member, low, high));
    }
    if compare_values(low, high) == Some(Ordering::Greater) {
        return Err(between_bounds_out_of_order(member, low, high));
    }
    Ok(())
}

/// How a refusal names the position past the last token of an expression.
pub const END_OF_EXPRESSION_TOKEN: &str = "<EOF>";

/// The refusal for an expression this service cannot parse: the token the parse
/// stopped at and the text around it, both taken from the expression the request sent.
pub fn expression_syntax_error_at(member: &str, token: &str, near: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!("Syntax error; token: \"{token}\", near: \"{near}\""),
    )
}

/// The same refusal for a CHARACTER the lexer cannot read: the character is the token,
/// and the text around it is the text the TOKEN window would quote.
///
/// A character the grammar has no token for is one token the expression should not carry,
/// so the window is the one every syntax refusal of this service states ([`token_window`],
/// [`ExprParser::near_text_at`]): the token before the offending one, the offending one
/// itself, and the token after it, quoted from the expression's OWN TEXT so the spacing is
/// the request's.
///
/// Stated over the CHARACTERS, because the lexer stopped before there were tokens: the text
/// of the token before the offending character is whatever non-whitespace runs up to it, so
/// the window opens at the start of the run the character sits in — and at the start of the
/// run BEFORE it where the character opens its own run. It closes at the end of that run
/// where text follows the character inside it, and at the end of the next run otherwise.
pub fn lexical_syntax_error(member: &str, characters: &[char], at: usize) -> RequestError {
    let at = at.min(characters.len());
    let token = characters
        .get(at)
        .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), |c| c.to_string());
    let run_start = characters[..at]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |index| index + 1);
    let run_end = characters
        .iter()
        .enumerate()
        .skip(at)
        .find(|(_, c)| c.is_whitespace())
        .map_or(characters.len(), |(index, _)| index);
    // The token BEFORE the offending character: the text ahead of it in its own run, or
    // the run before it where there is none.
    let start = if at > run_start {
        run_start
    } else {
        previous_run_start(characters, run_start)
    };
    // The token AFTER it: the text behind it in its own run, or the run after it where
    // there is none.
    let end = if at + 1 < run_end {
        run_end
    } else {
        next_run_end(characters, run_end)
    };
    let near: String = characters[start..end.max(start)].iter().collect();
    expression_syntax_error_at(member, &token, &near)
}

/// Where the last non-whitespace run before `run_start` begins, or `run_start` itself when
/// nothing but whitespace precedes it.
fn previous_run_start(characters: &[char], run_start: usize) -> usize {
    let Some(last) = characters[..run_start]
        .iter()
        .rposition(|c| !c.is_whitespace())
    else {
        return run_start;
    };
    characters[..=last]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |index| index + 1)
}

/// Where the first non-whitespace run after `run_end` ends, or `run_end` itself when
/// nothing but whitespace follows it.
fn next_run_end(characters: &[char], run_end: usize) -> usize {
    let Some(first) = characters[run_end..]
        .iter()
        .position(|c| !c.is_whitespace())
        .map(|offset| run_end + offset)
    else {
        return run_end;
    };
    characters
        .iter()
        .enumerate()
        .skip(first)
        .find(|(_, c)| c.is_whitespace())
        .map_or(characters.len(), |(index, _)| index)
}

/// The refusal for a BRACKETED LIST INDEX the lexer cannot read: the character where the
/// index stops being one is the token, and the text around it is that character with the
/// one before and the one after it.
///
/// An index is spelled in single characters — the `[`, the digits, the `]` — so the window
/// the parser's refusals state over the TOKENS around the offending one ([`token_window`])
/// is stated here over the characters around the offending one: a negative index is refused
/// at the sign that cannot start one, quoting the bracket, the sign and the digit after it.
/// Reporting the `[` instead would point at a bracket an index may legitimately open.
fn malformed_index(member: &str, characters: &[char], at: usize) -> RequestError {
    let token = characters
        .get(at)
        .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), char::to_string);
    let start = at.saturating_sub(1);
    let end = (at + 2).min(characters.len()).max(start);
    let near: String = characters
        .get(start..end)
        .map(|window| window.iter().collect())
        .unwrap_or_default();
    expression_syntax_error_at(member, &token, &near)
}

/// The refusal for a DynamoDB RESERVED WORD written out as an attribute name.
///
/// "To access an attribute whose name conflicts with a DynamoDB reserved word" is what
/// `ExpressionAttributeNames` is for (`UpdateItem$ExpressionAttributeNames`), so a bare
/// reserved word in an expression is read as the keyword it is and the expression is
/// refused — the caller has to supply the name through that map instead.
fn reserved_attribute_name(member: &str, word: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!("Attribute name is a reserved keyword; reserved keyword: {word}"),
    )
}

/// The refusal for a pair of parentheses that groups what is already grouped: fixed text
/// with no variable part — the sentence names neither the position of the pair nor the
/// expression, because the whole expression is the mistake.
pub fn redundant_parentheses(member: &str) -> RequestError {
    invalid_expression_member(member, "The expression has redundant parentheses;")
}

/// One token of an expression, as the REDUNDANT-PARENTHESIS rule reads it
/// ([`states_redundant_parentheses`]): the rule is about grouping alone, so every token
/// that neither opens nor closes a group is the same token to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupToken {
    Open,
    Close,
    Other,
    /// The `(` that carries a function CALL's arguments. It closes with a `)` like a group
    /// does, so the walk pairs it with one, and it groups NOTHING — what stands inside it
    /// are that call's arguments — so a pair written around one argument is not a pair
    /// around a group.
    Call,
    /// A token the grammar reads as a COMPARATOR: what tells a group that states an
    /// operand from a group that states a condition ([`group_states_an_operand`]). Which
    /// words a grammar compares with is its own vocabulary, so each grammar marks its own.
    Comparator,
}

/// Whether the group that opens at `at` states an OPERAND of a comparison rather than a
/// condition: the token after its closing `)` is a comparator.
///
/// Parentheses group whatever the grammar reads at the position they stand in, and both a
/// condition and an operand can stand where a condition begins, so the two are told apart
/// by what FOLLOWS the group — a comparator after it makes the group one side of that
/// comparison, since a condition is not compared against anything. A group the expression
/// never closes is no group to read either way, so it is left to the condition reading,
/// which reports where the text ran out.
///
/// Stated over the GROUPING alone, like the redundant-parenthesis rule beside it
/// ([`states_redundant_parentheses`]), so the condition and filter grammar and the
/// `KeyConditionExpression` grammar — which tokenize their own text and share no parser —
/// are held to one reading of the rule rather than two. A call's parentheses close with a
/// `)` like a group's do, so the walk pairs them with one just the same.
pub fn group_states_an_operand(tokens: &[GroupToken], at: usize) -> bool {
    let mut depth = 0usize;
    for (position, token) in tokens.iter().enumerate().skip(at) {
        match token {
            GroupToken::Open | GroupToken::Call => depth += 1,
            GroupToken::Close => {
                // A `)` closing nothing is unbalanced, which the grammar states on its
                // own: there is no group here to read as either.
                depth = match depth.checked_sub(1) {
                    Some(depth) => depth,
                    None => return false,
                };
                if depth == 0 {
                    return matches!(tokens.get(position + 1), Some(GroupToken::Comparator));
                }
            }
            GroupToken::Comparator | GroupToken::Other => {}
        }
    }
    false
}

/// Whether a token stream holds a pair of parentheses that groups what is already
/// grouped.
///
/// A pair that wraps nothing but another pair states nothing about how the expression is
/// read, and DynamoDB refuses it rather than reading past it. Stated over the GROUPING
/// alone, so the condition and filter grammar and the `KeyConditionExpression` grammar —
/// which tokenize their own text and share no parser — are held to one reading of the rule
/// rather than two.
///
/// The parentheses of a function CALL carry its arguments rather than grouping anything, so
/// they are read as the call's own ([`GroupToken::Call`]): neither the pair a call opens nor
/// a single pair written around one of its arguments states a group inside a group, while a
/// pair around such an argument's own pair states exactly that.
pub fn states_redundant_parentheses(tokens: &[GroupToken]) -> bool {
    // The index of the `(` each `)` closes, filled in as the walk closes each group, so
    // the group that ends just before a `)` is known when that `)` is reached.
    let mut opened_by: Vec<Option<usize>> = vec![None; tokens.len()];
    let mut open_groups: Vec<usize> = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        match token {
            GroupToken::Open | GroupToken::Call => open_groups.push(at),
            GroupToken::Close => {
                // A `)` closing nothing is unbalanced, which the grammar states on its
                // own: there is no group here to call redundant.
                let Some(open) = open_groups.pop() else {
                    return false;
                };
                opened_by[at] = Some(open);
                // A call's parentheses group nothing, so what they carry is never a group
                // inside a group: `f((a))` states one pair around one argument.
                if matches!(tokens.get(open), Some(GroupToken::Call)) {
                    continue;
                }
                // This group's content is exactly one group of its own: it begins where
                // that group begins and ends where it ends.
                if at > 0
                    && matches!(tokens.get(at - 1), Some(GroupToken::Close))
                    && opened_by[at - 1] == Some(open + 1)
                {
                    return true;
                }
            }
            GroupToken::Comparator | GroupToken::Other => {}
        }
    }
    false
}

/// The refusal for a function name the expression languages do not have.
///
/// The functions are a CLOSED set — "These function names are case-sensitive" and the
/// `ConditionExpression`/`UpdateExpression` docs list every one of them — so an
/// identifier called like a function that is not one of them is refused naming the
/// offending identifier, as the request spelled it, behind the expression member it
/// travelled in.
fn unknown_function(member: &str, name: &str) -> RequestError {
    invalid_expression_member(member, &format!("Invalid function name; function: {name}"))
}

/// Which GRAMMAR an expression is read under, which is what decides whether a function of
/// the expression languages may be called in it at all.
///
/// The two grammars this service reads expressions with: the one a `ConditionExpression`,
/// a `FilterExpression` and the legacy filter members state a condition in, and the one an
/// `UpdateExpression` states its actions in. Each defines its OWN functions — the
/// `ConditionExpression` function reference lists `attribute_exists`,
/// `attribute_not_exists`, `attribute_type`, `begins_with`, `contains` and `size`, and the
/// `UpdateItem$UpdateExpression` reference lists `if_not_exists` and `list_append` — so a
/// function of one grammar written in the other is a function that text may not call at
/// all ([`ExprParser::call_in_an_operand_position`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grammar {
    Condition,
    Update,
}

impl Grammar {
    /// How a refusal about a function names this grammar.
    fn named(self) -> &'static str {
        match self {
            Self::Condition => "a condition expression",
            Self::Update => "an update expression",
        }
    }

    /// Whether this grammar defines a function called `name`.
    fn defines(self, name: &str) -> bool {
        match self {
            // `size` is a function of the condition grammar whose value is an OPERAND of a
            // comparison rather than a condition of its own.
            Self::Condition => condition_function_signature(name).is_some() || name == SIZE_FUNCTION,
            Self::Update => matches!(name, "if_not_exists" | "list_append"),
        }
    }
}

/// The refusal a RECOGNISED function earns in a grammar that defines no such function,
/// naming the grammar and the function as the request spelled it.
fn function_not_allowed_in(member: &str, grammar: Grammar, name: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "The function is not allowed in {}; function: {name}",
            grammar.named()
        ),
    )
}

/// The refusal for a call this grammar has no function of that name for, told apart by
/// WHETHER the expression languages define a function by that name at all.
///
/// An identifier none of them defines is a name the grammar does not know
/// ([`unknown_function`]); one they DO define — `contains` written in an update, or
/// `if_not_exists` written in a condition — names a function of this service that this
/// grammar does not take, which is a different defect and told as such
/// ([`function_not_allowed_in`]), so a caller is pointed at the context rather than at a
/// spelling it got right.
fn function_outside_its_grammar(member: &str, grammar: Grammar, name: &str) -> RequestError {
    if function_signature(name).is_some() {
        return function_not_allowed_in(member, grammar, name);
    }
    unknown_function(member, name)
}

/// Whether a call to `name` states a CONDITION rather than a value.
///
/// The condition reference's functions each answer "true if …" — `attribute_exists`,
/// `attribute_not_exists`, `attribute_type`, `begins_with` and `contains` — so a call to
/// one of them states a condition and never a value, whichever grammar wrote it. The
/// remaining functions state values: `size (path)` "Returns a number representing an
/// attribute's size", and `if_not_exists`/`list_append` evaluate to the operand and the
/// list they compose (`UpdateItem$UpdateExpression`).
///
/// What that decides is HOW a call written where the grammar states an OPERAND is refused:
/// a function that states no value cannot fill any operand position, so it is refused for
/// the way it is used ([`function_used_this_way`]), while a value function written where a
/// document PATH belongs is refused for the path that position needs
/// ([`requires_document_path`]).
fn function_states_a_condition(name: &str) -> bool {
    condition_function_signature(name).is_some()
}

/// Whether a call to `inner` may stand where `enclosing`'s operand list states an
/// operand.
///
/// A function's operand is a value or a document path to read one from — "if_not_exists
/// (path, operand)" and "list_append (operand, operand)"
/// (`UpdateItem$UpdateExpression`) — and the ONE nesting that reference writes is the
/// one that fills a list that may not be there yet, `list_append`'s operand read through
/// `if_not_exists`. Every other call written in an operand position is a function used
/// where the grammar states none ([`function_used_this_way`]).
fn function_nests_in(inner: &str, enclosing: &str) -> bool {
    (inner, enclosing) == ("if_not_exists", "list_append")
}

/// The refusal a function used in a position this grammar does not state one in earns,
/// naming the function that is used that way as the request spelled it.
///
/// Told apart from a name the grammar does not know at all ([`unknown_function`]): the
/// identifier names one of this language's functions, and what is wrong is WHERE it was
/// written.
fn function_used_this_way(member: &str, name: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "The function is not allowed to be used this way in an expression; function: {name}"
        ),
    )
}

/// Whether a NAME an expression writes out is one of DynamoDB's reserved words, refused
/// behind the member it travelled in when it is.
///
/// The one place that rule is spelled, for the expression grammars this module parses
/// and for the `KeyConditionExpression` grammar the `Query` handler parses: both reach
/// an attribute whose name collides with a keyword through `ExpressionAttributeNames`
/// ("To access an attribute whose name conflicts with a DynamoDB reserved word" — that
/// member's doc), so a bare reserved word is refused in one wording wherever it is
/// written.
pub fn check_reserved_attribute_name(member: &str, word: &str) -> Result<(), RequestError> {
    if is_reserved_word(word) {
        return Err(reserved_attribute_name(member, word));
    }
    Ok(())
}

/// Whether `word`, written out in an expression, is one of DynamoDB's reserved words.
///
/// Matched without regard to case, the way the keywords of the expression languages are
/// ("The operator and section keywords are case-insensitive").
///
/// A word the expression language spells as one of its OWN FUNCTIONS is not held to the
/// list, however: the text names that function ([`function_signature`] holds every one of
/// them), so where the grammar wants a name the word is read as the attribute it names and
/// the request is served rather than refused — exactly as an operator keyword written in a
/// name position is read as the operator it spells ([`is_expression_operator_keyword`])
/// rather than as a reserved attribute name. The function names are matched AS WRITTEN,
/// because "These function names are case-sensitive" (the `ConditionExpression` function
/// reference), so a word that only differs from one in case names no function of this
/// language and the list above decides it.
fn is_reserved_word(word: &str) -> bool {
    if function_signature(word).is_some() {
        return false;
    }
    let upper = word.to_ascii_uppercase();
    RESERVED_WORDS.binary_search(&upper.as_str()).is_ok()
}

/// The lookup above is a binary search, which silently answers "not reserved" for every
/// word if the list below ever goes out of order — a missed reserved word is a request
/// wrongly accepted — so the order is a compile-time requirement rather than a
/// convention.
const _: () = {
    let mut at = 1;
    while at < RESERVED_WORDS.len() {
        assert!(
            str_before(RESERVED_WORDS[at - 1], RESERVED_WORDS[at]),
            "the reserved-word list must be sorted: the binary search below depends on it"
        );
        at += 1;
    }
};

/// Whether `left` sorts strictly before `right`, byte by byte — `str::cmp` is not
/// callable in a const context, so the comparison the assertion above needs is spelled
/// out here.
const fn str_before(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut at = 0;
    while at < left.len() && at < right.len() {
        if left[at] != right[at] {
            return left[at] < right[at];
        }
        at += 1;
    }
    left.len() < right.len()
}

/// DynamoDB's reserved words, from the "Reserved words in DynamoDB" list in the Amazon
/// DynamoDB Developer Guide, sorted so a lookup is a binary search rather than a scan
/// of the whole list per name an expression writes.
const RESERVED_WORDS: [&str; 573] = [
    "ABORT", "ABSOLUTE", "ACTION", "ADD", "AFTER", "AGENT", "AGGREGATE", "ALL", "ALLOCATE",
    "ALTER", "ANALYZE", "AND", "ANY", "ARCHIVE", "ARE", "ARRAY", "AS", "ASC", "ASCII",
    "ASENSITIVE", "ASSERTION", "ASYMMETRIC", "AT", "ATOMIC", "ATTACH", "ATTRIBUTE", "AUTH",
    "AUTHORIZATION", "AUTHORIZE", "AUTO", "AVG", "BACK", "BACKUP", "BASE", "BATCH", "BEFORE",
    "BEGIN", "BETWEEN", "BIGINT", "BINARY", "BIT", "BLOB", "BLOCK", "BOOLEAN", "BOTH",
    "BREADTH", "BUCKET", "BULK", "BY", "BYTE", "CALL", "CALLED", "CALLING", "CAPACITY",
    "CASCADE", "CASCADED", "CASE", "CAST", "CATALOG", "CHAR", "CHARACTER", "CHECK", "CLASS",
    "CLOB", "CLOSE", "CLUSTER", "CLUSTERED", "CLUSTERING", "CLUSTERS", "COALESCE", "COLLATE",
    "COLLATION", "COLLECTION", "COLUMN", "COLUMNS", "COMBINE", "COMMENT", "COMMIT", "COMPACT",
    "COMPILE", "COMPRESS", "CONDITION", "CONFLICT", "CONNECT", "CONNECTION", "CONSISTENCY",
    "CONSISTENT", "CONSTRAINT", "CONSTRAINTS", "CONSTRUCTOR", "CONSUMED", "CONTINUE", "CONVERT",
    "COPY", "CORRESPONDING", "COUNT", "COUNTER", "CREATE", "CROSS", "CUBE", "CURRENT", "CURSOR",
    "CYCLE", "DATA", "DATABASE", "DATE", "DATETIME", "DAY", "DEALLOCATE", "DEC", "DECIMAL",
    "DECLARE", "DEFAULT", "DEFERRABLE", "DEFERRED", "DEFINE", "DEFINED", "DEFINITION", "DELETE",
    "DELIMITED", "DEPTH", "DEREF", "DESC", "DESCRIBE", "DESCRIPTOR", "DETACH", "DETERMINISTIC",
    "DIAGNOSTICS", "DIRECTORIES", "DISABLE", "DISCONNECT", "DISTINCT", "DISTRIBUTE", "DO",
    "DOMAIN", "DOUBLE", "DROP", "DUMP", "DURATION", "DYNAMIC", "EACH", "ELEMENT", "ELSE",
    "ELSEIF", "EMPTY", "ENABLE", "END", "EQUAL", "EQUALS", "ERROR", "ESCAPE", "ESCAPED", "EVAL",
    "EVALUATE", "EXCEEDED", "EXCEPT", "EXCEPTION", "EXCEPTIONS", "EXCLUSIVE", "EXEC", "EXECUTE",
    "EXISTS", "EXIT", "EXPLAIN", "EXPLODE", "EXPORT", "EXPRESSION", "EXTENDED", "EXTERNAL",
    "EXTRACT", "FAIL", "FALSE", "FAMILY", "FETCH", "FIELDS", "FILE", "FILTER", "FILTERING",
    "FINAL", "FINISH", "FIRST", "FIXED", "FLATTERN", "FLOAT", "FOR", "FORCE", "FOREIGN",
    "FORMAT", "FORWARD", "FOUND", "FREE", "FROM", "FULL", "FUNCTION", "FUNCTIONS", "GENERAL",
    "GENERATE", "GET", "GLOB", "GLOBAL", "GO", "GOTO", "GRANT", "GREATER", "GROUP", "GROUPING",
    "HANDLER", "HASH", "HAVE", "HAVING", "HEAP", "HIDDEN", "HOLD", "HOUR", "IDENTIFIED",
    "IDENTITY", "IF", "IGNORE", "IMMEDIATE", "IMPORT", "IN", "INCLUDING", "INCLUSIVE",
    "INCREMENT", "INCREMENTAL", "INDEX", "INDEXED", "INDEXES", "INDICATOR", "INFINITE",
    "INITIALLY", "INLINE", "INNER", "INNTER", "INOUT", "INPUT", "INSENSITIVE", "INSERT",
    "INSTEAD", "INT", "INTEGER", "INTERSECT", "INTERVAL", "INTO", "INVALIDATE", "IS",
    "ISOLATION", "ITEM", "ITEMS", "ITERATE", "JOIN", "KEY", "KEYS", "LAG", "LANGUAGE", "LARGE",
    "LAST", "LATERAL", "LEAD", "LEADING", "LEAVE", "LEFT", "LENGTH", "LESS", "LEVEL", "LIKE",
    "LIMIT", "LIMITED", "LINES", "LIST", "LOAD", "LOCAL", "LOCALTIME", "LOCALTIMESTAMP",
    "LOCATION", "LOCATOR", "LOCK", "LOCKS", "LOG", "LOGED", "LONG", "LOOP", "LOWER", "MAP",
    "MATCH", "MATERIALIZED", "MAX", "MAXLEN", "MEMBER", "MERGE", "METHOD", "METRICS", "MIN",
    "MINUS", "MINUTE", "MISSING", "MOD", "MODE", "MODIFIES", "MODIFY", "MODULE", "MONTH",
    "MULTI", "MULTISET", "NAME", "NAMES", "NATIONAL", "NATURAL", "NCHAR", "NCLOB", "NEW",
    "NEXT", "NO", "NONE", "NOT", "NULL", "NULLIF", "NUMBER", "NUMERIC", "OBJECT", "OF",
    "OFFLINE", "OFFSET", "OLD", "ON", "ONLINE", "ONLY", "OPAQUE", "OPEN", "OPERATOR", "OPTION",
    "OR", "ORDER", "ORDINALITY", "OTHER", "OTHERS", "OUT", "OUTER", "OUTPUT", "OVER",
    "OVERLAPS", "OVERRIDE", "OWNER", "PAD", "PARALLEL", "PARAMETER", "PARAMETERS", "PARTIAL",
    "PARTITION", "PARTITIONED", "PARTITIONS", "PATH", "PERCENT", "PERCENTILE", "PERMISSION",
    "PERMISSIONS", "PIPE", "PIPELINED", "PLAN", "POOL", "POSITION", "PRECISION", "PREPARE",
    "PRESERVE", "PRIMARY", "PRIOR", "PRIVATE", "PRIVILEGES", "PROCEDURE", "PROCESSED",
    "PROJECT", "PROJECTION", "PROPERTY", "PROVISIONING", "PUBLIC", "PUT", "QUERY", "QUIT",
    "QUORUM", "RAISE", "RANDOM", "RANGE", "RANK", "RAW", "READ", "READS", "REAL", "REBUILD",
    "RECORD", "RECURSIVE", "REDUCE", "REF", "REFERENCE", "REFERENCES", "REFERENCING", "REGEXP",
    "REGION", "REINDEX", "RELATIVE", "RELEASE", "REMAINDER", "RENAME", "REPEAT", "REPLACE",
    "REQUEST", "RESET", "RESIGNAL", "RESOURCE", "RESPONSE", "RESTORE", "RESTRICT", "RESULT",
    "RETURN", "RETURNING", "RETURNS", "REVERSE", "REVOKE", "RIGHT", "ROLE", "ROLES",
    "ROLLBACK", "ROLLUP", "ROUTINE", "ROW", "ROWS", "RULE", "RULES", "SAMPLE", "SATISFIES",
    "SAVE", "SAVEPOINT", "SCAN", "SCHEMA", "SCOPE", "SCROLL", "SEARCH", "SECOND", "SECTION",
    "SEGMENT", "SEGMENTS", "SELECT", "SELF", "SEMI", "SENSITIVE", "SEPARATE", "SEQUENCE",
    "SERIALIZABLE", "SESSION", "SET", "SETS", "SHARD", "SHARE", "SHARED", "SHORT", "SHOW",
    "SIGNAL", "SIMILAR", "SIZE", "SKEWED", "SMALLINT", "SNAPSHOT", "SOME", "SOURCE", "SPACE",
    "SPACES", "SPARSE", "SPECIFIC", "SPECIFICTYPE", "SPLIT", "SQL", "SQLCODE", "SQLERROR",
    "SQLEXCEPTION", "SQLSTATE", "SQLWARNING", "START", "STATE", "STATIC", "STATUS", "STORAGE",
    "STORE", "STORED", "STREAM", "STRING", "STRUCT", "STYLE", "SUB", "SUBMULTISET",
    "SUBPARTITION", "SUBSTRING", "SUBTYPE", "SUM", "SUPER", "SYMMETRIC", "SYNONYM", "SYSTEM",
    "TABLE", "TABLESAMPLE", "TEMP", "TEMPORARY", "TERMINATED", "TEXT", "THAN", "THEN",
    "THROUGHPUT", "TIME", "TIMESTAMP", "TIMEZONE", "TINYINT", "TO", "TOKEN", "TOTAL", "TOUCH",
    "TRAILING", "TRANSACTION", "TRANSFORM", "TRANSLATE", "TRANSLATION", "TREAT", "TRIGGER",
    "TRIM", "TRUE", "TRUNCATE", "TTL", "TUPLE", "TYPE", "UNDER", "UNDO", "UNION", "UNIQUE",
    "UNIT", "UNKNOWN", "UNLOGGED", "UNNEST", "UNPROCESSED", "UNSIGNED", "UNTIL", "UPDATE",
    "UPPER", "URL", "USAGE", "USE", "USER", "USERS", "USING", "UUID", "VACUUM", "VALUE",
    "VALUED", "VALUES", "VARCHAR", "VARIABLE", "VARIANCE", "VARINT", "VARYING", "VIEW",
    "VIEWS", "VIRTUAL", "VOID", "WAIT", "WHEN", "WHENEVER", "WHERE", "WHILE", "WINDOW", "WITH",
    "WITHIN", "WITHOUT", "WORK", "WRAPPED", "WRITE", "YEAR", "ZONE",
];

/// The expression text around the token at `at`: the token before it, the token itself
/// (there is none past the end) and the token after, as the expression wrote them.
fn token_window(tokens: &[ExprToken], at: usize) -> String {
    let before = at.checked_sub(1).and_then(|index| tokens.get(index));
    let here = tokens.get(at);
    let after = tokens.get(at + 1);
    [before, here, after]
        .into_iter()
        .flatten()
        .map(token_text)
        .collect::<Vec<String>>()
        .join(" ")
}

/// One token as the expression wrote it, which is what the two refusals above quote.
fn token_text(token: &ExprToken) -> String {
    match token {
        ExprToken::Word(word) => word.clone(),
        ExprToken::Alias(alias) => alias.clone(),
        ExprToken::Placeholder(placeholder) => placeholder.clone(),
        ExprToken::Index(index) => format!("[{index}]"),
        ExprToken::Compare(operator) => operator.as_str().to_string(),
        ExprToken::Open => "(".to_string(),
        ExprToken::Close => ")".to_string(),
        ExprToken::Comma => ",".to_string(),
        ExprToken::Dot => ".".to_string(),
        ExprToken::Plus => "+".to_string(),
        ExprToken::Minus => "-".to_string(),
    }
}

/// Split one expression into tokens, refusing a text or a token count past the bounds
/// above before any parsing starts.
pub fn tokenize_expression(
    member: &str,
    expression: &str,
) -> Result<Vec<ExprToken>, RequestError> {
    tokenize_expression_spanned(member, expression).map(|read| read.tokens)
}

/// One expression as it was READ: its tokens, its characters, and where in those
/// characters each token sits.
///
/// The spans are what lets a refusal quote the expression as the REQUEST spelled it
/// ([`ExprParser::near_text`]) rather than re-rendering its tokens: `a == :v` and
/// `a = = :v` are the same tokens and different text, and the text is what the caller
/// sent and has to find the mistake in.
pub struct TokenizedExpression {
    pub tokens: Vec<ExprToken>,
    characters: Vec<char>,
    spans: Vec<(usize, usize)>,
}

impl TokenizedExpression {
    /// The characters and token spans a parser quotes the source through.
    fn source(&self) -> ExpressionSource<'_> {
        ExpressionSource {
            characters: &self.characters,
            spans: &self.spans,
        }
    }
}

/// [`tokenize_expression`], keeping where each token sat in the expression's text.
pub fn tokenize_expression_spanned(
    member: &str,
    expression: &str,
) -> Result<TokenizedExpression, RequestError> {
    if expression.len() > MAX_EXPRESSION_BYTES {
        return Err(expression_size_exceeded(member));
    }
    let characters: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        let character = characters[at];
        let opened = tokens.len();
        let start = at;
        match character {
            c if c.is_whitespace() => at += 1,
            '(' => {
                tokens.push(ExprToken::Open);
                at += 1;
            }
            ')' => {
                tokens.push(ExprToken::Close);
                at += 1;
            }
            ',' => {
                tokens.push(ExprToken::Comma);
                at += 1;
            }
            '.' => {
                tokens.push(ExprToken::Dot);
                at += 1;
            }
            '+' => {
                tokens.push(ExprToken::Plus);
                at += 1;
            }
            '-' => {
                tokens.push(ExprToken::Minus);
                at += 1;
            }
            '=' => {
                tokens.push(ExprToken::Compare(CompareOp::Equal));
                at += 1;
            }
            '<' | '>' => {
                let next = characters.get(at + 1).copied();
                let (operator, width) = match (character, next) {
                    ('<', Some('=')) => (CompareOp::LessOrEqual, 2),
                    ('<', Some('>')) => (CompareOp::NotEqual, 2),
                    ('<', _) => (CompareOp::Less, 1),
                    (_, Some('=')) => (CompareOp::GreaterOrEqual, 2),
                    _ => (CompareOp::Greater, 1),
                };
                tokens.push(ExprToken::Compare(operator));
                at += width;
            }
            '[' => {
                let start = at + 1;
                let mut end = start;
                while end < characters.len() && characters[end].is_ascii_digit() {
                    end += 1;
                }
                if end == start || characters.get(end) != Some(&']') {
                    return Err(malformed_index(member, &characters, end));
                }
                let digits: String = characters[start..end].iter().collect();
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| malformed_index(member, &characters, start))?;
                tokens.push(ExprToken::Index(index));
                at = end + 1;
            }
            '#' | ':' => {
                let start = at;
                at += 1;
                while at < characters.len() && is_expression_word_char(characters[at]) {
                    at += 1;
                }
                if at - start < 2 {
                    return Err(lexical_syntax_error(member, &characters, start));
                }
                let text: String = characters[start..at].iter().collect();
                tokens.push(if character == '#' {
                    ExprToken::Alias(text)
                } else {
                    ExprToken::Placeholder(text)
                });
            }
            c if is_expression_word_char(c) => {
                let start = at;
                while at < characters.len() && is_expression_word_char(characters[at]) {
                    at += 1;
                }
                tokens.push(ExprToken::Word(characters[start..at].iter().collect()));
            }
            _ => return Err(lexical_syntax_error(member, &characters, at)),
        }
        // Exactly one token per pass, or none for a run of whitespace.
        if tokens.len() > opened {
            spans.push((start, at));
        }
    }
    if tokens.is_empty() {
        // An expression member carrying nothing but whitespace states no expression at
        // all, which is a different fact from one whose text the parser stopped inside:
        // AWS says the member is empty rather than pointing at a token.
        return Err(expression_is_empty(member));
    }
    // How much the expression ASKS FOR, as the documented quota counts it: the operators and
    // functions it states ([`MAX_EXPRESSION_OPERATORS`]). Held here, where every expression
    // member is read, and before any of them is parsed — the count is a fact of the text.
    //
    // It is read off the WHOLE token stream, and so before this emulator's own ceiling on
    // how many tokens it keeps: the operator quota is the documented rule a caller is
    // answered by, and it REPORTS the count the expression stated, which an expression cut
    // short at the ceiling could not say. The walk above is bounded whatever the ceiling is
    // — a token costs at least one character of a text already held to
    // [`MAX_EXPRESSION_BYTES`] — so nothing unbounded is read to count them.
    let operators = stated_operators(&tokens);
    if operators > MAX_EXPRESSION_OPERATORS {
        return Err(too_many_expression_operators(member, operators));
    }
    if tokens.len() > MAX_EXPRESSION_TOKENS {
        return Err(RequestError::Validation(expression_clause(
            member,
            &format!("an expression may hold at most {MAX_EXPRESSION_TOKENS} tokens"),
        )));
    }
    Ok(TokenizedExpression {
        tokens,
        characters,
        spans,
    })
}

/// How a member past [`MAX_EXPRESSION_OPERATORS`] is refused: the member it was sent as and
/// the count it stated, inside the envelope that member's refusals carry
/// ([`envelopes_expression_refusals`]).
fn too_many_expression_operators(member: &str, operators: usize) -> RequestError {
    expression_refusal(
        envelopes_expression_refusals(member),
        expression_clause(
            member,
            &format!("The expression contains too many operators; operator count: {operators}"),
        ),
    )
}

/// The words this grammar reads as OPERATORS rather than as names: the logical
/// connectives, the membership test and the range comparator the condition and filter
/// languages are written with ("AND", "OR", "NOT", "IN", "BETWEEN" — the operators
/// `Query$FilterExpression` and `ConditionExpression` spell).
///
/// An operator keyword is a token of the grammar wherever the text spells it, so it is
/// never an attribute NAME: a position that wants a name or an operand has no place for
/// one, and the parse stops AT it ([`ExprParser::path_name`]) rather than reading it as a
/// name and stopping at whatever follows. That is why they are counted as the operators
/// they are ([`stated_operators`]) too, and why an attribute actually called one of them
/// is reached through `ExpressionAttributeNames`, which is what that member is for.
pub(super) const EXPRESSION_OPERATOR_KEYWORDS: [&str; 5] =
    ["AND", "OR", "NOT", "IN", BETWEEN_OPERATOR];

/// Whether a bare word is one of those operator keywords. The operator keywords are
/// case-insensitive, as [`ExprParser::eat_keyword`] matches them.
pub(super) fn is_expression_operator_keyword(word: &str) -> bool {
    EXPRESSION_OPERATOR_KEYWORDS
        .iter()
        .any(|keyword| word.eq_ignore_ascii_case(keyword))
}

/// How many operators and functions the tokens of one expression state, the way the quota
/// counts them ([`MAX_EXPRESSION_OPERATORS`]).
///
/// One per comparison (`=`, `<>`, `<`, `<=`, `>`, `>=`), per arithmetic operator an update
/// computes with (`+`, `-`), per logical operator (`AND`, `OR`, `NOT`), per membership or
/// range operator (`IN`, `BETWEEN`) and per function CALL — a word the text applies to an
/// operand list, which is what the `(` after it makes it.
///
/// The OPERANDS of an operator are not operators: the values an `IN` list carries are
/// operands separated by commas, so `a IN (b, c, d)` states the one operator the quota's own
/// example counts in it, however long the list is. For the same reason the `AND` that
/// separates a `BETWEEN` range's two bounds belongs to that `BETWEEN` rather than being a
/// logical operator of its own.
fn stated_operators(tokens: &[ExprToken]) -> usize {
    let mut operators = 0;
    // The `BETWEEN`s whose range `AND` has not been read yet, which is what makes each of
    // them claim exactly one `AND`.
    let mut ranges = 0usize;
    for (at, token) in tokens.iter().enumerate() {
        match token {
            ExprToken::Compare(_) | ExprToken::Plus | ExprToken::Minus => operators += 1,
            ExprToken::Word(_) if matches!(tokens.get(at + 1), Some(ExprToken::Open)) => {
                operators += 1;
            }
            ExprToken::Word(word) => match word.to_ascii_uppercase().as_str() {
                BETWEEN_OPERATOR => {
                    operators += 1;
                    ranges += 1;
                }
                "AND" if ranges > 0 => ranges -= 1,
                other if is_expression_operator_keyword(other) => operators += 1,
                _ => {}
            },
            _ => {}
        }
    }
    operators
}

/// The substitution tables an expression resolves its names and values through:
/// `ExpressionAttributeNames` and `ExpressionAttributeValues`, exactly as the request
/// sent them.
#[derive(Debug, Clone, Copy, Default)]
pub struct Substitutions<'a> {
    names: Option<&'a HashMap<String, String>>,
    values: Option<&'a HashMap<String, AttributeValue>>,
}

impl<'a> Substitutions<'a> {
    pub fn new(
        names: Option<&'a HashMap<String, String>>,
        values: Option<&'a HashMap<String, AttributeValue>>,
    ) -> Self {
        Self { names, values }
    }

    /// The attribute name an `#alias` stands for. An alias with no entry is the
    /// caller's mistake, not an empty name.
    pub fn name(&self, alias: &str) -> Result<String, RequestError> {
        self.names
            .and_then(|names| names.get(alias))
            .cloned()
            .ok_or_else(|| RequestError::Validation(undefined_name_clause(alias)))
    }

    /// The value a `:placeholder` stands for.
    pub fn value(&self, placeholder: &str) -> Result<AttributeValue, RequestError> {
        self.values
            .and_then(|values| values.get(placeholder))
            .cloned()
            .ok_or_else(|| RequestError::Validation(undefined_value_clause(placeholder)))
    }

    /// Hold the request's expressions and both substitution tables to each other: every
    /// token an expression DEREFERENCES must be one the tables define, and every token
    /// the tables define must be one an expression REFERENCES. The whole request is
    /// refused when either side has a token the other does not.
    ///
    /// Both members exist to be substituted INTO — they hold "substitution tokens for
    /// attribute names in an expression" and "values that can be substituted in an
    /// expression" (those members' docs) — so a token no expression names states
    /// nothing, and DynamoDB refuses the request rather than ignoring it. One entry
    /// point for every operation that takes expression members, so the rule does not
    /// drift between them.
    ///
    /// PRECEDENCE: a token the expression dereferences and no table defines is what the
    /// caller is told about FIRST, before any surplus entry, because it is the mistake
    /// that makes the expression unreadable — dropping a surplus entry would leave the
    /// request refused for the undefined token anyway. It is stated through the member
    /// that carried the expression, in the same words the parser that resolves the token
    /// for real states it in, so one request earns one sentence whichever check reaches
    /// it first.
    ///
    /// `expressions` pairs each expression member of the request with its value as it
    /// was sent; an expression this does not tokenize is left to the parser that reads
    /// it for real, which refuses it with the syntax error the member earns.
    pub fn check_all_referenced(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        self.check_tables_well_formed(expressions)?;
        self.check_tokens_referenced(expressions)
    }

    /// The same rules, where the reader of one of those members answers the UNDEFINED tokens
    /// of its own text itself.
    ///
    /// A member whose reader decides which of the defects its own text states is answered —
    /// the defects an operand can carry, read in the order the expression wrote them — has
    /// already reported an undefined token it writes FIRST, and is holding one it writes after
    /// a defect of its own behind that defect. So this pass collects the tokens such a member
    /// references, which is what the unused-entry rule needs, without reporting one the
    /// request does not define: doing so would answer a defect written LATER in place of the
    /// one the member's text states first.
    ///
    /// Every other member is held to the rule exactly as [`Substitutions::check_all_referenced`]
    /// holds it.
    pub fn check_all_referenced_outside(
        &self,
        expressions: &[(&str, Option<&str>)],
        answered_by_their_own_reader: &[&str],
    ) -> Result<(), RequestError> {
        self.check_tables_well_formed(expressions)?;
        self.check_tokens_referenced_outside(expressions, answered_by_their_own_reader)
    }

    /// The first half of that rule: what each substitution table the request SENT says
    /// about itself, read without looking at any expression's text — its keys are tokens
    /// an expression could write, the request carries an expression they could be
    /// substituted into, and the table defines something.
    ///
    /// Split from the second half because the two sit on either side of the bounds the
    /// request's VALUES are held to: a table that is not a substitution table at all is
    /// the defect whatever the request's values hold, while a value outside its documented
    /// limits is outside them whether or not an expression references the entry it sits
    /// under. The two writers that bound their values themselves call the halves in that
    /// order; every other caller asks for both at once
    /// ([`Substitutions::check_all_referenced`]).
    pub fn check_tables_well_formed(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        let enveloped = declares_enveloped_expression(expressions);
        self.check_table_keys(enveloped)?;
        self.check_an_expression_was_sent(expressions)?;
        self.check_tables_not_empty(enveloped)
    }

    /// The same rules over the expressions of ONE ELEMENT of a multi-item request.
    ///
    /// What each table SAYS is held to the same rules wherever the member sits: its keys
    /// are substitution tokens, it is not empty, and every token it defines is one the
    /// expressions beside it reference. What differs is the one rule that is about the ASK
    /// the members state together — that a table was sent with an expression to be
    /// substituted into at all ([`Substitutions::check_an_expression_was_sent`]): the
    /// tables of one `TransactItems` action sit beside the expression members of that
    /// action, which state only PART of what the element does (its condition), so an
    /// element that states no expression is admitted rather than refused for the
    /// expression it did not state or for the entries that expression would not have
    /// referenced.
    ///
    /// A `BatchGetItem` entry is such an element too: the members of one `RequestItems`
    /// entry are that table's own read — its projection, the legacy list that states the
    /// same ask and the names table that resolves the projection's aliases — and they are
    /// members of the ENTRY rather than of the call, so the entry is held to the element
    /// scope through [`Substitutions::check_all_referenced_in_element`] exactly as a
    /// transactional action's are.
    pub(super) fn check_referenced_within_element(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        let enveloped = declares_enveloped_expression(expressions);
        self.check_table_keys(enveloped)?;
        self.check_tables_not_empty(enveloped)?;
        if !names_an_expression(expressions, |_| true) {
            return Ok(());
        }
        self.check_tokens_referenced(expressions)
    }

    /// Every token either side names must be one the other has: an `#alias` or
    /// `:placeholder` an expression dereferences defined by the table it reads, and an
    /// entry a table defines referenced by one of the expressions.
    ///
    /// The second half of [`Substitutions::check_all_referenced`], which the two writers
    /// that bound their values themselves ask for after those values are bounded.
    pub fn check_tokens_referenced(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        self.check_tokens_referenced_outside(expressions, &[])
    }

    /// That half, where the members in `answered_by_their_own_reader` report the undefined
    /// tokens of their own text themselves
    /// ([`Substitutions::check_all_referenced_outside`]): their tokens still count as
    /// REFERENCED, and one they do not define is left to that reader.
    fn check_tokens_referenced_outside(
        &self,
        expressions: &[(&str, Option<&str>)],
        answered_by_their_own_reader: &[&str],
    ) -> Result<(), RequestError> {
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut values: BTreeSet<String> = BTreeSet::new();
        for (member, expression) in expressions {
            let Some(expression) = expression else {
                continue;
            };
            let answered_elsewhere = answered_by_their_own_reader.contains(member);
            let Ok(tokens) = tokenize_expression(member, expression) else {
                // An expression this cannot read tells nothing about which tokens it
                // references, and the parser that reads it for real refuses it with the
                // syntax error the member earns — which is the refusal the caller gets.
                return Ok(());
            };
            for token in tokens {
                match token {
                    ExprToken::Alias(alias) => {
                        if !answered_elsewhere && self.name(&alias).is_err() {
                            return Err(undefined_name(member, &alias));
                        }
                        names.insert(alias);
                    }
                    ExprToken::Placeholder(placeholder) => {
                        if !answered_elsewhere && self.value(&placeholder).is_err() {
                            return Err(undefined_value(member, &placeholder));
                        }
                        values.insert(placeholder);
                    }
                    _ => {}
                }
            }
        }
        // The sentence follows the expression member this request states its intent
        // through, like every other refusal raised about its expressions.
        let enveloped = expressions.iter().any(|(member, expression)| {
            expression.is_some() && envelopes_expression_refusals(member)
        });
        check_referenced("ExpressionAttributeNames", self.names, &names, enveloped)?;
        check_referenced("ExpressionAttributeValues", self.values, &values, enveloped)
    }

    /// A substitution table sent with NO expression to substitute into is a different
    /// defect from one whose entries an expression does not reference, and AWS words the
    /// two differently.
    ///
    /// Both members exist to be substituted INTO — they hold "substitution tokens for
    /// attribute names in an expression" and "values that can be substituted in an
    /// expression" (those members' docs) — so a table the request has no expression for has
    /// nothing to be substituted into: the caller has to ADD the expression, which is what
    /// this sentence says, where the unused-entry sentence ([`check_referenced`]) says to
    /// DROP the entry. Collapsing the two onto the second one tells a caller that supplied
    /// no expression that its names are unused in expressions.
    ///
    /// WHICH expressions count differs per table: a `#alias` stands for an attribute NAME,
    /// which every expression member spells, while a `:placeholder` stands for a VALUE,
    /// which only a member that COMPARES one can substitute — a projection names
    /// attributes and compares nothing ([`substitutes_values`]).
    ///
    /// A table that DEFINES something is what this is about: an empty one offers no token to
    /// substitute anywhere, so it is refused for being empty
    /// ([`Substitutions::check_tables_not_empty`], asked after this) rather than for the
    /// expression it has nothing to go into. That is also why this runs FIRST: a request
    /// that offers real tokens and no expression for them is told about the expression,
    /// whichever of its other tables happens to be empty.
    ///
    /// The sentence is entirely fixed — it names neither the keys nor an expression member,
    /// because there is no expression member to name. WHETHER it carries the
    /// `<N> validation error(s) detected: ` envelope follows the same rule every other
    /// refusal about this request's expressions follows ([`envelopes_expression_refusals`]):
    /// the members a WRITE states its intent through carry it and the members a READ narrows
    /// its result with do not — read off the members the OPERATION declares, since none of
    /// them was sent. The NAMES table is reported first, which is the order the two tables
    /// are checked in throughout.
    ///
    /// This rule is the REQUEST's own and is not asked of one ELEMENT of a TRANSACTIONAL
    /// request, whose tables sit beside expression members stating only part of what that
    /// element does ([`Substitutions::check_referenced_within_element`]).
    ///
    /// `expressions` is therefore read in the scope the CALL states its asks in. For an
    /// operation whose expression members are its own, that is the request's own members. A
    /// batched read states its projection inside each `RequestItems` entry while the
    /// GENERATIONS of its parameters are read over the whole call — one entry's legacy list
    /// collides with another entry's `ProjectionExpression` — so there it is what the call
    /// states anywhere rather than what the entry carrying the table states.
    pub(crate) fn check_an_expression_was_sent(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        let enveloped = declares_enveloped_expression(expressions);
        if defines_a_token(self.names) && !names_an_expression(expressions, |_| true) {
            return Err(only_with_expressions(enveloped, "ExpressionAttributeNames"));
        }
        if defines_a_token(self.values) && !names_an_expression(expressions, substitutes_values) {
            return Err(only_with_expressions(enveloped, "ExpressionAttributeValues"));
        }
        Ok(())
    }

    /// Every KEY a substitution table defines must BE a substitution token.
    ///
    /// Each table is read by the SPELLING of its keys — `ExpressionAttributeNames` holds
    /// "one or more substitution tokens for attribute names in an expression" and its keys
    /// "must begin with a `#`", and `ExpressionAttributeValues` the same for values with a
    /// `:` — so a key spelled any other way is a token no expression can write, and it is
    /// refused as the invalid key of that member it is. That is its OWN defect: collapsing
    /// it onto the unused-entry sentence would tell a caller to drop an entry whose
    /// spelling is the mistake, and collapsing it onto the undefined-token sentence would
    /// point at the expression instead of at the table.
    ///
    /// Asked FIRST, before what the table could be substituted into
    /// ([`Substitutions::check_an_expression_was_sent`]) and before any expression is read:
    /// a key that is no token names nothing whichever expressions the request carried. The
    /// NAMES table is reported first, which is the order the two tables are checked in
    /// throughout.
    fn check_table_keys(&self, enveloped: bool) -> Result<(), RequestError> {
        check_substitution_keys(NAMES_MEMBER, self.names, NAME_TOKEN_PREFIX, enveloped)?;
        check_substitution_keys(VALUES_MEMBER, self.values, VALUE_TOKEN_PREFIX, enveloped)
    }

    /// The same check, for a reader that reads an expression BEFORE it holds the request's
    /// tables to its expressions ([`Substitutions::check_all_referenced`]): a key that is
    /// no token defines nothing, so it is settled before the expression that would
    /// dereference one is read.
    ///
    /// `members` are the expression members that reader reads, which is what decides
    /// whether the refusal carries the validation envelope
    /// ([`envelopes_expression_refusals`]).
    pub fn check_keys_are_tokens(&self, members: &[&str]) -> Result<(), RequestError> {
        self.check_table_keys(members.iter().copied().any(envelopes_expression_refusals))
    }

    /// A substitution table the request SENT must define something, and every name it
    /// defines must BE a name.
    ///
    /// Both members exist to be substituted into, so a table sent with no entries
    /// states nothing an expression could use: it is a member whose value breaks the
    /// rule for that member, which is what the validation envelope reports. A request
    /// that sends no table at all is a different thing and is not refused here —
    /// absence is how an expression that needs no substitution is spelled.
    ///
    /// An entry of the NAMES table whose value is empty names no attribute at all, so
    /// the token could resolve to nothing: the table carries an invalid value, and the
    /// refusal says which key it sits under.
    ///
    /// Both sentences carry the validation envelope exactly where every other refusal
    /// about this request's expressions does ([`envelopes_expression_refusals`]).
    fn check_tables_not_empty(&self, enveloped: bool) -> Result<(), RequestError> {
        if self.names.is_some_and(HashMap::is_empty) {
            return Err(empty_substitution_table(enveloped, "ExpressionAttributeNames"));
        }
        if self.values.is_some_and(HashMap::is_empty) {
            return Err(empty_substitution_table(
                enveloped,
                "ExpressionAttributeValues",
            ));
        }
        // One stable order over a map, so the same request earns the same message.
        let mut empty: Vec<&str> = self
            .names
            .into_iter()
            .flatten()
            .filter(|(_, name)| name.is_empty())
            .map(|(alias, _)| alias.as_str())
            .collect();
        empty.sort_unstable();
        if let Some(alias) = empty.first() {
            return Err(expression_refusal(
                enveloped,
                format!(
                    "ExpressionAttributeNames contains invalid value: Empty attribute name for \
                     key {alias}"
                ),
            ));
        }
        Ok(())
    }
}

/// Whether the expression members a request DECLARES carry the
/// `<N> validation error(s) detected: ` envelope — read off the members the operation
/// declares rather than off the ones it sent, because the refusals below are about a
/// request that sent none of them ([`envelopes_expression_refusals`] is the rule).
fn declares_enveloped_expression(expressions: &[(&str, Option<&str>)]) -> bool {
    expressions
        .iter()
        .any(|(member, _)| envelopes_expression_refusals(member))
}

/// The refusal for a substitution table sent with no expression to substitute into: AWS's
/// FIXED sentence for that defect, naming the table alone, inside the validation envelope
/// where the operation's expression members carry one ([`expression_refusal`]).
pub(super) fn only_with_expressions(enveloped: bool, member: &str) -> RequestError {
    expression_refusal(
        enveloped,
        format!("{member} can only be specified when using expressions"),
    )
}

/// The member whose keys stand for attribute NAMES, and the character they are spelled
/// with.
const NAMES_MEMBER: &str = "ExpressionAttributeNames";
const NAME_TOKEN_PREFIX: char = '#';

/// The member whose keys stand for attribute VALUES, and the character they are spelled
/// with.
const VALUES_MEMBER: &str = "ExpressionAttributeValues";
const VALUE_TOKEN_PREFIX: char = ':';

/// The longest a KEY of either substitution table may be, 255 bytes: the documented limit
/// on a substitution token, counted over the whole key as the request spelled it — the
/// member's prefix character included, which is why a 255-character name under a `#` is one
/// byte too long.
///
/// It bounds the KEY and not the attribute name or value behind it: the key is the token an
/// expression writes, so what the limit is about is how long that token may be.
pub const MAX_SUBSTITUTION_KEY_BYTES: usize = 255;

/// Refuse one substitution table whose keys are not all tokens an expression can write:
/// each key is inside the length a token may have ([`MAX_SUBSTITUTION_KEY_BYTES`]), and
/// each is spelled as the token the member substitutes it into.
///
/// Both defects are the member's "invalid key", and either is reported ahead of any
/// expression being read ([`Substitutions::check_table_keys`]): a key that no expression
/// could write defines nothing, so what the caller is told about is the KEY rather than the
/// expression that failed to resolve a token it does not define. The keys are walked in one
/// stable order and the FIRST offending one is named, with the rule it broke: its LENGTH
/// first, because a key past the limit is past it whatever characters it spells — a caller
/// that over-ran the limit is told so rather than told about a character inside the text it
/// has to shorten anyway.
fn check_substitution_keys<V>(
    member: &str,
    table: Option<&HashMap<String, V>>,
    prefix: char,
    enveloped: bool,
) -> Result<(), RequestError> {
    let mut keys: Vec<&str> = table
        .into_iter()
        .flatten()
        .map(|(key, _)| key.as_str())
        .filter(|key| is_oversized_key(key) || !is_substitution_token(key, prefix))
        .collect();
    keys.sort_unstable();
    let Some(key) = keys.first() else {
        return Ok(());
    };
    let defect = if is_oversized_key(key) {
        // The one figure the sentence carries is the size of the offending key, measured
        // off the request: a caller that over-ran the limit is told by how much.
        format!(
            "The expression attribute map contains a key that is too long; size of key: {}",
            key.len()
        )
    } else {
        format!("Syntax error; key: \"{key}\"")
    };
    Err(expression_refusal(
        enveloped,
        format!("{member} contains invalid key: {defect}"),
    ))
}

/// Whether one key of a substitution table is longer than a substitution token may be
/// ([`MAX_SUBSTITUTION_KEY_BYTES`]), counted in BYTES as the request sent them.
fn is_oversized_key(key: &str) -> bool {
    key.len() > MAX_SUBSTITUTION_KEY_BYTES
}

/// Whether one key of a substitution table is spelled as the token an expression writes
/// for it: the member's own prefix, and then a name over the alphabet every expression
/// grammar of this service reads a token's name in ([`is_expression_word_char`]).
fn is_substitution_token(key: &str, prefix: char) -> bool {
    key.strip_prefix(prefix)
        .is_some_and(|name| !name.is_empty() && name.chars().all(is_expression_word_char))
}

/// Whether this request names an expression member `substitutable` admits, with a value.
fn names_an_expression(
    expressions: &[(&str, Option<&str>)],
    substitutable: fn(&str) -> bool,
) -> bool {
    expressions
        .iter()
        .any(|(member, expression)| expression.is_some() && substitutable(member))
}

/// Whether a substitution table the request sent DEFINES a token at all.
///
/// A table sent EMPTY offers no token to substitute anywhere, so it is refused for being
/// empty ([`Substitutions::check_tables_not_empty`]) rather than for the expression it has
/// nothing to go into.
fn defines_a_token<V>(table: Option<&HashMap<String, V>>) -> bool {
    table.is_some_and(|entries| !entries.is_empty())
}

/// Whether `member` is an expression a VALUE can be substituted into.
///
/// Every expression member spells attribute names, so a `#alias` has somewhere to go in any
/// of them; a `:placeholder` stands for a value to compare or to write, and a projection
/// does neither — "one or more attributes to be retrieved" (`ProjectionExpression`) names
/// attributes and compares nothing — so a value token sent to a request whose only
/// expression is a projection has nothing to be substituted into.
fn substitutes_values(member: &str) -> bool {
    member != PROJECTION_EXPRESSION_MEMBER
}

/// The expression member that names attributes to report rather than values to compare.
const PROJECTION_EXPRESSION_MEMBER: &str = "ProjectionExpression";

/// The refusal for a substitution table sent with no entries: AWS's own sentence, naming
/// the member, inside the validation envelope where the operation's expression members
/// carry one ([`expression_refusal`]).
pub(super) fn empty_substitution_table(enveloped: bool, member: &str) -> RequestError {
    expression_refusal(enveloped, format!("{member} must not be empty"))
}

/// Refuse one substitution table that defines a key no expression referenced.
///
/// The keys are reported in one stable order — the order the map sorts in — so the
/// same request earns the same message every time, and the refusal names the member
/// that carried them.
///
/// This rule is the SERVICE's own, checked once the request validator has already
/// passed a well-formed input, so the sentence is stated on its own and carries the
/// `<N> validation error(s) detected: ` envelope only where every other refusal about
/// this request's expressions does — `enveloped`, decided by
/// [`envelopes_expression_refusals`] over the members the request carried.
fn check_referenced<V>(
    member: &str,
    supplied: Option<&HashMap<String, V>>,
    referenced: &BTreeSet<String>,
    enveloped: bool,
) -> Result<(), RequestError> {
    let unused: BTreeSet<&str> = supplied
        .into_iter()
        .flatten()
        .map(|(key, _)| key.as_str())
        .filter(|key| !referenced.contains(*key))
        .collect();
    if unused.is_empty() {
        return Ok(());
    }
    Err(expression_refusal(
        enveloped,
        format!(
            "Value provided in {member} unused in expressions: keys: {{{}}}",
            unused.into_iter().collect::<Vec<&str>>().join(", ")
        ),
    ))
}

/// One step of a document path: an attribute name, or an index into a list.
///
/// Ordered so that two paths of one update can be ordered step by step — an index by
/// its number and an attribute by its name — which is what lets the removals of one
/// update be applied greatest-first (see [`ParsedUpdate::apply`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathStep {
    Attribute(String),
    Index(usize),
}

/// A document path, as an expression writes it — `a`, `a.b`, `a[2].b` — with every
/// `#alias` already resolved. The first step is always an attribute name, so every
/// path starts at one of the item's own attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentPath(pub(crate) Vec<PathStep>);

impl DocumentPath {
    /// The path naming one top-level attribute, which is all the legacy
    /// `AttributeUpdates`/`Expected` maps can address.
    pub fn attribute_named(name: &str) -> Self {
        Self(vec![PathStep::Attribute(name.to_string())])
    }

    /// The top-level attribute this path reaches into, which is the attribute a write
    /// through it changes.
    pub fn attribute(&self) -> &str {
        match self.0.first() {
            Some(PathStep::Attribute(name)) => name.as_str(),
            // Unreachable by construction: every path starts at an attribute.
            _ => "",
        }
    }

    /// The whole path as ONE top-level attribute, when that is all it is: `a` yes, `a.b`
    /// and `a[0]` no. What tells a path that READS an attribute from one that reaches
    /// inside it.
    pub fn single_attribute(&self) -> Option<&str> {
        match self.0.as_slice() {
            [PathStep::Attribute(name)] => Some(name.as_str()),
            _ => None,
        }
    }

    /// How the path reads, for a refusal message and for the prefix index two actions
    /// are checked for overlap through.
    fn render(&self) -> String {
        self.render_prefix(self.0.len())
    }

    /// The same for this path's first `steps` steps only: a path overlaps another
    /// exactly when one of them is one of the other's step-prefixes.
    fn render_prefix(&self, steps: usize) -> String {
        let mut text = String::new();
        for step in self.0.iter().take(steps) {
            match step {
                PathStep::Attribute(name) => {
                    if !text.is_empty() {
                        text.push('.');
                    }
                    text.push_str(name);
                }
                PathStep::Index(index) => {
                    text.push('[');
                    text.push_str(&index.to_string());
                    text.push(']');
                }
            }
        }
        text
    }

    /// The same path as AWS's refusals print it: the steps as a comma-separated token
    /// list, an index keeping its brackets — `l[1]` is `[l, [1]]` and `m.a.b` is
    /// `[m, a, b]`.
    fn render_tokens(&self) -> String {
        self.render_tokens_prefix(self.0.len())
    }

    /// The same for this path's first `steps` steps only, which is what the overlap
    /// refusal states for the path that contains the other.
    fn render_tokens_prefix(&self, steps: usize) -> String {
        let tokens: Vec<String> = self
            .0
            .iter()
            .take(steps)
            .map(|step| match step {
                PathStep::Attribute(name) => name.clone(),
                PathStep::Index(index) => format!("[{index}]"),
            })
            .collect();
        format!("[{}]", tokens.join(", "))
    }

    /// The refusal for a path that does not address a location this item can hold.
    ///
    /// AWS's text for this condition is FIXED: it states that the path is invalid and
    /// nothing of which path it was, so a client matching the sentence exactly — the
    /// normal pattern for a constant message — recognises the condition.
    fn invalid(&self) -> RequestError {
        RequestError::Validation(INVALID_DOCUMENT_PATH_MESSAGE.to_string())
    }

    /// Whether every step of this path BUT THE LAST addresses a container in `item` that
    /// the last step can read: a map for a name step and a list for an index step.
    ///
    /// What tells a location the item does not hold YET — nothing to remove, nothing to
    /// delete from, a map entry a `SET` creates — from a path that addresses no location at
    /// all, because a step before the last reaches nothing or reaches a value of the wrong
    /// kind. Only the second is the request's mistake
    /// ([`INVALID_DOCUMENT_PATH_MESSAGE`]); a path naming one top-level attribute always
    /// addresses the item itself, so an attribute that is not there is the first.
    pub fn addresses_a_location(&self, item: &Item) -> bool {
        let Some((last, parents)) = self.0.split_last() else {
            return false;
        };
        if parents.is_empty() {
            return matches!(last, PathStep::Attribute(_));
        }
        matches!(
            (last, Self(parents.to_vec()).get(item)),
            (PathStep::Attribute(_), Some(AttributeValue::M(_)))
                | (PathStep::Index(_), Some(AttributeValue::L(_)))
        )
    }

    /// The value at this path, or `None` when the item holds nothing there.
    pub fn get<'i>(&self, item: &'i Item) -> Option<&'i AttributeValue> {
        let mut steps = self.0.iter();
        let PathStep::Attribute(first) = steps.next()? else {
            return None;
        };
        let mut current = item.get(first)?;
        for step in steps {
            current = match (step, current) {
                (PathStep::Attribute(name), AttributeValue::M(entries)) => entries.get(name)?,
                (PathStep::Index(index), AttributeValue::L(values)) => values.get(*index)?,
                _ => return None,
            };
        }
        Some(current)
    }

    /// Write `value` at this path, creating a missing map entry and appending past the
    /// end of a list, but never inventing a missing parent container.
    pub fn set(&self, item: &mut Item, value: AttributeValue) -> Result<(), RequestError> {
        let Some((PathStep::Attribute(first), rest)) = self.0.split_first() else {
            return Err(self.invalid());
        };
        if rest.is_empty() {
            item.insert(first.clone(), value);
            return Ok(());
        }
        let parent = item.get_mut(first).ok_or_else(|| self.invalid())?;
        set_within(parent, rest, value).map_err(|()| self.invalid())
    }

    /// Remove whatever this path addresses, reporting what was there.
    pub fn remove(&self, item: &mut Item) -> Option<AttributeValue> {
        let (head, rest) = self.0.split_first()?;
        let PathStep::Attribute(first) = head else {
            return None;
        };
        if rest.is_empty() {
            return item.remove(first);
        }
        remove_within(item.get_mut(first)?, rest)
    }
}

/// Write `value` at `steps` inside `target`.
///
/// Recursive, and bounded: a path carries at most [`MAX_ATTRIBUTE_DEPTH`] steps (the
/// parser refuses a longer one), so the number of frames is a constant of this service
/// rather than a function of the request.
fn set_within(
    target: &mut AttributeValue,
    steps: &[PathStep],
    value: AttributeValue,
) -> Result<(), ()> {
    match (steps.first(), target) {
        (None, slot) => {
            *slot = value;
            Ok(())
        }
        (Some(PathStep::Attribute(name)), AttributeValue::M(entries)) => {
            if steps.len() == 1 {
                entries.insert(name.clone(), value);
                Ok(())
            } else {
                set_within(entries.get_mut(name).ok_or(())?, &steps[1..], value)
            }
        }
        (Some(PathStep::Index(index)), AttributeValue::L(values)) => {
            if steps.len() == 1 {
                // "the new element is appended to the end of the list" when the index
                // is past its end, which is what DynamoDB does with an out-of-range
                // index in a SET.
                if *index < values.len() {
                    values[*index] = value;
                } else {
                    values.push(value);
                }
                Ok(())
            } else {
                set_within(values.get_mut(*index).ok_or(())?, &steps[1..], value)
            }
        }
        _ => Err(()),
    }
}

/// Remove what `steps` addresses inside `target`, bounded the same way
/// [`set_within`] is. Removing a list element shifts the ones after it down.
fn remove_within(target: &mut AttributeValue, steps: &[PathStep]) -> Option<AttributeValue> {
    match (steps.first()?, target) {
        (PathStep::Attribute(name), AttributeValue::M(entries)) => {
            if steps.len() == 1 {
                entries.remove(name)
            } else {
                remove_within(entries.get_mut(name)?, &steps[1..])
            }
        }
        (PathStep::Index(index), AttributeValue::L(values)) => {
            if steps.len() == 1 {
                (*index < values.len()).then(|| values.remove(*index))
            } else {
                remove_within(values.get_mut(*index)?, &steps[1..])
            }
        }
        _ => None,
    }
}

/// Whether two attribute values are the same value.
///
/// Comparison is by TYPE first, as the `ComparisonOperator` doc's `EQ` entry states:
/// "If an item contains an AttributeValue element of a different type than the one
/// provided in the request, the value does not match. For example, {"S":"6"} does not
/// equal {"N":"6"}". Within a type, two `N` values compare NUMERICALLY (`6` equals
/// `6.0`), a set compares as a set — its members in any order — and a list or map
/// compares element by element.
pub fn attribute_values_equal(left: &AttributeValue, right: &AttributeValue) -> bool {
    values_equal_within(left, right, MAX_ATTRIBUTE_DEPTH)
}

/// The same, with an explicit depth budget: a stored value nests at most
/// [`MAX_ATTRIBUTE_DEPTH`] levels (every write path refuses a deeper one), so the walk
/// cannot recurse further than that however the two values are shaped.
fn values_equal_within(left: &AttributeValue, right: &AttributeValue, budget: usize) -> bool {
    if budget == 0 {
        return false;
    }
    match (left, right) {
        (AttributeValue::S(mine), AttributeValue::S(theirs)) => mine == theirs,
        (AttributeValue::B(mine), AttributeValue::B(theirs)) => mine == theirs,
        (AttributeValue::N(mine), AttributeValue::N(theirs)) => numbers_equal(mine, theirs),
        (AttributeValue::Bool(mine), AttributeValue::Bool(theirs)) => mine == theirs,
        (AttributeValue::Null(mine), AttributeValue::Null(theirs)) => mine == theirs,
        (AttributeValue::Ss(mine), AttributeValue::Ss(theirs)) => {
            same_members(mine, theirs, |text| text.as_str())
        }
        (AttributeValue::Ns(mine), AttributeValue::Ns(theirs)) => {
            same_members(mine, theirs, |text| canonical_number(text))
        }
        (AttributeValue::Bs(mine), AttributeValue::Bs(theirs)) => {
            same_members(mine, theirs, |bytes| bytes.as_slice())
        }
        (AttributeValue::L(mine), AttributeValue::L(theirs)) => {
            mine.len() == theirs.len()
                && mine
                    .iter()
                    .zip(theirs.iter())
                    .all(|(a, b)| values_equal_within(a, b, budget - 1))
        }
        (AttributeValue::M(mine), AttributeValue::M(theirs)) => {
            mine.len() == theirs.len()
                && mine.iter().all(|(name, value)| {
                    theirs
                        .get(name)
                        .is_some_and(|other| values_equal_within(value, other, budget - 1))
                })
        }
        _ => false,
    }
}

/// Whether two `N` texts denote one value, which is a numeric question rather than a
/// textual one. A text neither side can parse falls back to the text itself.
fn numbers_equal(left: &str, right: &str) -> bool {
    match (NumberKey::parse(left), NumberKey::parse(right)) {
        (Some(mine), Some(theirs)) => mine == theirs,
        _ => left == right,
    }
}

/// The refusal for an update that states more actions than this service will index,
/// apply and write for one request.
fn too_many_actions(count: usize) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: an update may state at most \
         {MAX_UPDATE_ACTIONS} actions, got {count}"
    ))
}

/// AWS's fixed text for an update whose document path addresses no location the item
/// can hold — a step through a parent that is not there.
pub const INVALID_DOCUMENT_PATH_MESSAGE: &str =
    "The document path provided in the update expression is invalid for update";

/// AWS's fixed text for an update action whose operand is a type the STORED attribute
/// cannot be combined with — an `ADD` of one set type onto another, an `ADD` of a set
/// onto a number, an `ADD` or `DELETE` against an attribute that is neither a number
/// nor a set.
///
/// It names NEITHER operand: the sentence AWS states for this class carries no
/// attribute path and no `AttributeValue` type tag, so the types the request and the
/// stored item happened to carry are not interpolated into it. An operand the OPERATOR
/// itself cannot take, whatever the item holds, is the refusal that names the operator
/// and its type set instead ([`incorrect_update_operand_type_for`]).
pub const INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE: &str =
    "An operand in the update expression has an incorrect data type";

/// The refusal [`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`] states, which every update
/// action raises for an operand whose type it cannot be applied to.
fn incorrect_update_operand_type() -> RequestError {
    RequestError::Validation(INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE.to_string())
}

/// The member an update written as an EXPRESSION travels in, which is the member whose
/// refusals name the operator the expression wrote.
const UPDATE_EXPRESSION_MEMBER: &str = "UpdateExpression";

/// The member an update written as the LEGACY per-attribute map travels in.
pub const ATTRIBUTE_UPDATES_MEMBER: &str = "AttributeUpdates";

/// Whether an update stated through `member` is one whose `ADD` CONCATENATES a list
/// operand onto the list the attribute holds.
///
/// Only the legacy per-attribute map does: its `ADD` is defined over "the data type of
/// the attribute" (`AttributeValueUpdate$Action`) and recorded behaviour includes a list
/// there, while the expression member states a closed operand domain of its own — "The
/// ADD action only supports Number and set data types" (`UpdateItem$UpdateExpression`).
fn appends_lists(member: &str) -> bool {
    member == ATTRIBUTE_UPDATES_MEMBER
}

/// The data type an operand-type refusal names, in the vocabulary that family of
/// messages spells types with: the `AttributeValue` member's own type as the union spells
/// it, one capitalised word per part of the documented type name — "An attribute of type
/// String Set" (`AttributeValue$SS`) is `StringSet`, `B` is `Binary`, `L` is `List` —
/// rather than the union member TAG a type-mismatch clause reports
/// ([`crate::state::attribute_value_type`]).
fn operand_type_name(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::S(_) => "String",
        AttributeValue::N(_) => "Number",
        AttributeValue::B(_) => "Binary",
        AttributeValue::Ss(_) => "StringSet",
        AttributeValue::Ns(_) => "NumberSet",
        AttributeValue::Bs(_) => "BinarySet",
        AttributeValue::M(_) => "Map",
        AttributeValue::L(_) => "List",
        AttributeValue::Null(_) => "Null",
        AttributeValue::Bool(_) => "Boolean",
    }
}

/// What ONE update operator may be given as its value operand: the operator keyword a
/// refusal names, the NAME of the domain that operator admits, and which values are in it.
///
/// The domain is a property of the OPERATOR, not of the item: an operand outside it
/// cannot be applied whatever the attribute holds, so it is refused naming the operator
/// and that operator's own type set ([`ValueOperand::check`]), while an operand INSIDE the
/// domain that does not match the type the item holds is the class refusal
/// ([`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`]) the two set walkers raise.
///
/// The type set is a FIELD and not one constant for both operators: the two actions admit
/// different types — "The ADD action only supports Number and set data types" and "The
/// DELETE action only supports set data types" (`UpdateItem$UpdateExpression`) — so each
/// names the domain it holds its operand to, and a refusal cannot point a caller at the
/// other operator's rule.
struct ValueOperand {
    operator: &'static str,
    type_set: &'static str,
    takes: fn(&AttributeValue) -> bool,
}

impl ValueOperand {
    /// Hold one operand to this operator's domain, refusing it with the operator, its
    /// own type and this operator's type set when it is outside it.
    fn check(&self, member: &str, value: &AttributeValue) -> Result<(), RequestError> {
        if (self.takes)(value) {
            return Ok(());
        }
        Err(incorrect_update_operand_type_for(
            member,
            self.operator,
            self.type_set,
            value,
        ))
    }
}

/// `ADD`'s operand domain: "The ADD action only supports Number and set data types"
/// (`UpdateItem$UpdateExpression`), so a number or one of the three set types.
const ADD_OPERAND: ValueOperand = ValueOperand {
    operator: "ADD",
    type_set: "ALLOWED_FOR_ADD_OPERAND",
    takes: |value| {
        matches!(
            value,
            AttributeValue::N(_)
                | AttributeValue::Ns(_)
                | AttributeValue::Ss(_)
                | AttributeValue::Bs(_)
        )
    },
};

/// `DELETE`'s operand domain: "The DELETE action only supports set data types" (the same
/// doc), so one of the three set types and nothing else.
const DELETE_OPERAND: ValueOperand = ValueOperand {
    operator: "DELETE",
    type_set: "ALLOWED_FOR_DELETE_OPERAND",
    takes: |value| {
        matches!(
            value,
            AttributeValue::Ss(_) | AttributeValue::Ns(_) | AttributeValue::Bs(_)
        )
    },
};

/// Whether a request that states its update through `member` WRITES AN OPERATOR, which
/// is what an operand-type refusal can name.
///
/// The one place that question is answered, beside the one that answers which members
/// carry the validation envelope ([`envelopes_expression_refusals`]): an update written
/// as an expression spells `ADD` or `DELETE` inside the text it carries, while the legacy
/// `AttributeUpdates` map names its action per attribute instead — so a refusal about one
/// of its values is stated as a defect of that parameter value
/// ([`unsupported_action_type`]) rather than of an operand of an expression.
fn writes_an_operator(member: &str) -> bool {
    member == UPDATE_EXPRESSION_MEMBER
}

/// The refusal for an update operand of a type its operator cannot take at all: AWS
/// names the OPERATOR the expression wrote, the offending operand's own type name and
/// the name of the type set that operator allows, each read off the request, behind the
/// expression member that carried them.
///
/// Stated that way for a member that writes an operator ([`writes_an_operator`]). The
/// LEGACY per-attribute map writes no operator inside a text — it names its action per
/// attribute — so AWS reports the same fact as a defect of the parameter VALUE that
/// entry carried: the action it asked for and the `AttributeValue` type that value is,
/// both read off the request ([`unsupported_action_type`]).
fn incorrect_update_operand_type_for(
    member: &str,
    operator: &str,
    type_set: &str,
    value: &AttributeValue,
) -> RequestError {
    if !writes_an_operator(member) {
        return unsupported_action_type(operator, value);
    }
    invalid_expression_member(
        member,
        &format!(
            "Incorrect operand type for operator or function; operator: {operator}, operand \
             type: {}, typeSet: {type_set}",
            operand_type_name(value)
        ),
    )
}

/// The refusal a LEGACY `AttributeUpdates` entry earns whose value is a type its action
/// is not defined over: the action the entry named and the union member tag of the value
/// it carried, inside the validation envelope every refusal about a value an input
/// member carried is stated in.
///
/// A fact of the REQUEST alone — the entry's `Action` and the type of its `Value` — so
/// it is the same refusal whatever the addressed item holds at that attribute, and
/// whether it holds anything at all. The type is spelled with the tag the
/// `AttributeValue` union names that member with ([`attribute_value_type`]), which is
/// the vocabulary this family of clauses spells a type with.
fn unsupported_action_type(action: &str, value: &AttributeValue) -> RequestError {
    refused(format!(
        "One or more parameter values were invalid: {action} action is not supported for the \
         type {}",
        attribute_value_type(value)
    ))
}

/// AWS's fixed text for an update operand that reads an attribute the item does not
/// carry: the sentence names the class of mistake and not the path that was read, which
/// is the one place a caller would otherwise read a path out of this message.
pub const MISSING_OPERAND_ATTRIBUTE_MESSAGE: &str =
    "The provided expression refers to an attribute that does not exist in the item";

/// The refusal for two actions of one update that write the same document location, or
/// one inside the other: DynamoDB cannot apply both.
///
/// The two paths are stated in the ORDER THE UPDATE STATES THEM — `path one` is the
/// path of the earlier action and `path two` the later one, whichever of them is the
/// deeper — because the message is a report of what the caller wrote. Two actions on
/// the SAME location state that one location twice, which reads alike either way round.
fn overlapping_paths(member: &str, one: &str, two: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "Two document paths overlap with each other; must remove or rewrite one of these \
             paths; path one: {one}, path two: {two}"
        ),
    )
}

/// The refusal for two document paths that disagree about the KIND of container one
/// location is: one steps into it by NAME and the other by INDEX.
///
/// A map lookup reads a map and a list index reads a list, so two paths that step through
/// the same location in both ways cannot both be about one item — the location is a map or
/// a list and not both — and DynamoDB refuses the pair rather than reading one of them
/// against a value of the other kind. The two paths are stated WHOLE and in the order the
/// expression states them, exactly as the overlap sentence beside this one states them
/// ([`overlapping_paths`]).
fn conflicting_paths(member: &str, one: &str, two: &str) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "Two document paths conflict with each other; must remove or rewrite one of these \
             paths; path one: {one}, path two: {two}"
        ),
    )
}

/// Refuse a list of document paths in which one path addresses a location inside — or
/// the same as — another's, in the order the expression stated them.
///
/// The rule holds wherever one expression names several locations: two actions of an
/// update cannot both be applied, and two paths of a projection cannot both be reported.
///
/// The check INDEXES the paths rather than comparing every pair: a path overlaps another
/// exactly when one is a step-prefix of the other, so each path only has to look its own
/// prefixes up. The index keeps each path's POSITION in the expression, because that is
/// what orders the two paths in the refusal ([`overlapping_paths`]).
///
/// Two paths that are neither one inside the other can still DISAGREE about one location,
/// where they step through it in two ways — a name step into a map and an index step into
/// a list — which is the second rule a pair of paths is held to ([`check_paths_agree`]).
pub(super) fn check_paths_disjoint(
    member: &'static str,
    paths: &[&DocumentPath],
) -> Result<(), RequestError> {
    let mut stated: BTreeMap<String, usize> = BTreeMap::new();
    for (position, path) in paths.iter().enumerate() {
        if stated.insert(path.render(), position).is_some() {
            return Err(overlapping_paths(
                member,
                &path.render_tokens(),
                &path.render_tokens(),
            ));
        }
    }
    for (position, path) in paths.iter().enumerate() {
        for steps in 1..path.0.len() {
            let prefix = path.render_prefix(steps);
            if let Some(stated_prefix) = stated.get(&prefix) {
                let whole = path.render_tokens();
                let prefix = path.render_tokens_prefix(steps);
                // The path stated EARLIER in the expression is `path one`.
                let (one, two) = if *stated_prefix < position {
                    (prefix, whole)
                } else {
                    (whole, prefix)
                };
                return Err(overlapping_paths(member, &one, &two));
            }
        }
    }
    check_paths_agree(member, paths)
}

/// Refuse a list of document paths two of which step through ONE location in two
/// different ways: by attribute name, which reads a map, and by index, which reads a list.
///
/// The paths are indexed by the location each step reaches rather than compared pairwise:
/// the prefix of steps BEFORE a step is the location that step reads, so a location two
/// paths disagree about is found by keeping, per prefix, the kind of the step the first
/// path took from it ([`conflicting_paths`] states the pair). Each path contributes at most
/// one entry per step, and a path carries at most [`MAX_ATTRIBUTE_DEPTH`] steps.
fn check_paths_agree(member: &'static str, paths: &[&DocumentPath]) -> Result<(), RequestError> {
    // Per location — the steps that lead to it — the first path that stepped from it, and
    // whether it stepped by INDEX. The path stated EARLIER in the expression is `path one`,
    // which is the one kept here.
    let mut stepped: BTreeMap<&[PathStep], (&DocumentPath, bool)> = BTreeMap::new();
    for path in paths {
        for (steps, step) in path.0.iter().enumerate() {
            let indexed = matches!(step, PathStep::Index(_));
            let reached = &path.0[..steps];
            match stepped.get(reached) {
                Some((first, stepped_indexed)) if *stepped_indexed != indexed => {
                    return Err(conflicting_paths(
                        member,
                        &first.render_tokens(),
                        &path.render_tokens(),
                    ));
                }
                Some(_) => {}
                None => {
                    stepped.insert(reached, (path, indexed));
                }
            }
        }
    }
    Ok(())
}

/// The refusal for an operand whose type the operator or function it was given to does
/// not take: AWS names the operator or function and the type of the offending operand,
/// both read off the request.
fn incorrect_operand_type(
    member: &str,
    operator_or_function: &str,
    operand_type: &str,
) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "Incorrect operand type for operator or function; operator or function: \
             {operator_or_function}, operand type: {operand_type}"
        ),
    )
}

/// The limits every attribute value this service stores — or compares a stored one
/// against — is held to: the documented nesting depth, the documented 400 KB item size
/// (no ONE value can be larger than the item that must hold it),
/// [`MAX_COLLECTION_MEMBERS`] over the sets, lists and maps the value carries, and the
/// documented rules for the value's own TYPE (the numeric domain and the set rules).
///
/// Every term comes from the request, and every one bounds work done per request later
/// (the walk over a value, the clone of it into the item, the set and list operations
/// over its members), so every value is held to them at admission — on every write
/// path, for every substituted expression value, AND for every value an update computes
/// before it is written.
///
/// The TYPE rules are checked here as well, so they hold for every value a request
/// carries and not only for the ones a write stores: a number outside the documented
/// precision or exponent range, and a set that is empty or lists one value twice, are
/// refused whether the value arrived in an item, in `ExpressionAttributeValues`, in a
/// statement's `Parameters` or in a key.
pub fn check_attribute_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    check_value_within_the_item_limits(name, value)?;
    // No ONE value can be larger than the item that would have to hold it, so this is
    // also the bound on the bytes a single substitution can carry into the store: the
    // check PRECEDES every clone of it.
    if attribute_value_size(value) > MAX_ITEM_SIZE_BYTES {
        return Err(RequestError::Validation(
            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// Every one of those limits EXCEPT the item ceiling: the documented nesting depth,
/// [`MAX_COLLECTION_MEMBERS`], and the rules for the value's own type.
fn check_value_within_the_item_limits(
    name: &str,
    value: &AttributeValue,
) -> Result<(), RequestError> {
    // The NAME first: a name no attribute can be stored under is refused before anything
    // about the value it was given, because the name is what the value would be held at.
    if carries_an_over_long_attribute_name(name, value) {
        return Err(refused(attribute_name_too_large_message(None)));
    }
    if attribute_depth_exceeds(value, MAX_ATTRIBUTE_DEPTH) {
        return Err(nesting_levels_exceeded());
    }
    if value_members(value) > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members(name, value_members(value)));
    }
    check_value_rules(value)?;
    Ok(())
}

/// Whether this request carried an attribute NAME no attribute could be stored under — one
/// longer than [`MAX_ATTRIBUTE_NAME_BYTES`] — in `name` itself or in any MAP KEY the value
/// nests.
///
/// The model declares that bound on the shape every member carrying an attribute name
/// targets (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeName")`),
/// and the framework's validating walk reports a violation of it on a map's VALUES only: it
/// treats a map's KEY as a segment of the path it reports a violation under, so no walk
/// ahead of a handler can report the key at all. The keys of every map a request carries
/// are therefore held to the bound HERE, where every value a request carries is admitted —
/// an item's attributes, a nested map inside one, a value an expression binds, a parameter a
/// statement carries — so one rule answers for all of them rather than one check per
/// member.
///
/// The walk is ITERATIVE, over an explicit stack, for the reason [`value_members`] gives.
/// Length is counted in BYTES, which is the unit the sentence states the bound in.
fn carries_an_over_long_attribute_name(name: &str, value: &AttributeValue) -> bool {
    if name.len() > MAX_ATTRIBUTE_NAME_BYTES {
        return true;
    }
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            AttributeValue::L(values) => pending.extend(values.iter()),
            AttributeValue::M(entries) => {
                for (key, held) in entries {
                    if key.len() > MAX_ATTRIBUTE_NAME_BYTES {
                        return true;
                    }
                    pending.push(held);
                }
            }
            _ => {}
        }
    }
    false
}

/// The same limits for a value an expression of a READ binds — a `Scan`'s or a
/// `Query`'s `ExpressionAttributeValues` entry — stated as its own sentence.
///
/// Which FORM a refusal takes follows the member the request states its intent through
/// ([`envelopes_expression_refusals`]): the members a write states its intent through
/// carry the `<N> validation error(s) detected: ` envelope, and the members a read
/// narrows its result with do not. The rules themselves are the same
/// ([`check_attribute_value`]).
pub fn check_bare_attribute_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    check_attribute_value(name, value).map_err(without_envelope)
}

/// The limits a value a request BINDS to a substitution token is held to: every rule
/// [`check_attribute_value`] states EXCEPT the documented item ceiling.
///
/// Binding a value stores nothing. A `ConditionExpression` operand is only COMPARED
/// against the stored item, an operand a `DELETE` names takes members away rather than
/// adding any, and an action that resolves to a no-op writes no byte at all — so a bound
/// value is not item bytes and the item ceiling is not a bound on it. Whether a bound
/// value fits an item is decided where the item that would hold it is sized: over the
/// values an update would STORE ([`ParsedUpdate::from_actions`]) and over the item the
/// change leaves behind ([`update_size_exceeded`]). What bounds the table as a WHOLE is
/// its own cap ([`check_substitution_total`]).
pub fn check_substituted_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    check_value_within_the_item_limits(name, value)
}

/// The same limits for a value an expression of a READ binds, stated as its own sentence
/// ([`check_bare_attribute_value`] is where that split is explained).
pub fn check_bare_substituted_value(
    name: &str,
    value: &AttributeValue,
) -> Result<(), RequestError> {
    check_substituted_value(name, value).map_err(without_envelope)
}

/// Hold one request's `ExpressionAttributeValues` to the cap on what the member may
/// carry in TOTAL ([`MAX_EXPRESSION_ATTRIBUTE_VALUES_BYTES`]).
///
/// The cap is on the SUM over the table's entries, so it bounds a request that spreads
/// its bytes over many tokens exactly as it bounds one that puts them all in a single
/// value — which is what makes it the bound on the substitution work one request can ask
/// for, whichever expressions dereference the tokens.
pub fn check_substitution_total(
    values: Option<&HashMap<String, AttributeValue>>,
) -> Result<(), RequestError> {
    let total = sum_of(
        values
            .into_iter()
            .flatten()
            .map(|(_token, value)| attribute_value_size(value)),
    );
    if total > MAX_EXPRESSION_ATTRIBUTE_VALUES_BYTES {
        return Err(refused(
            EXPRESSION_ATTRIBUTE_VALUES_TOO_LARGE_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// The same limits for a value a STATEMENT carries — a `?` parameter or a literal the
/// statement spells — stated as its own sentence rather than inside the validation
/// envelope, and WITHOUT the item ceiling.
///
/// A value a statement carries is not an item: whether it FITS one is decided where the
/// item it would be written into is sized, which is also what decides how that refusal is
/// worded — an `INSERT` states the ceiling the way a request carrying an oversized item is
/// refused, and an `UPDATE` the way a change that would grow the stored item past it is
/// ([`ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE`]). A `SELECT` writes no item at all, so a value
/// it compares against is bounded by what it IS and not by what an item could hold. Every
/// other rule is the same ([`check_attribute_value`]), and so is the FORM: the statement
/// surface answers the clause on its own, the way it answers every other refusal about a
/// statement, and states no separator for a value it has nothing to echo after it.
///
/// One rule is this surface's own: the statement surface COERCES a number through a
/// narrower reader than the item API does, so an exponent outside the integer that reader
/// uses is a parameter it cannot convert to a numeric value rather than a magnitude outside
/// the documented range ([`check_statement_number_width`]). It is asked FIRST, because a
/// text this surface cannot read is not a magnitude it can then range-check.
pub fn check_statement_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    check_statement_number_width(value)
        .and_then(|()| check_value_within_the_item_limits(name, value))
        .map_err(without_envelope)
        .map_err(without_empty_echo)
}

/// A clause whose echoed value was empty, with the separator that would have introduced
/// it dropped: the statement surface states the sentence alone rather than one ending in
/// a dangling `: `.
fn without_empty_echo(error: RequestError) -> RequestError {
    match error {
        RequestError::Validation(message) => RequestError::Validation(
            message
                .strip_suffix(": ")
                .map_or(message.clone(), str::to_string),
        ),
        other => other,
    }
}

/// One clause taken back out of the validation envelope [`validation_message`] put it
/// in: the inverse of that rendering, for a surface that states its clauses bare.
fn without_envelope(error: RequestError) -> RequestError {
    const ONE_CLAUSE: &str = "1 validation error detected: ";
    match error {
        RequestError::Validation(message) => RequestError::Validation(
            message
                .strip_prefix(ONE_CLAUSE)
                .map_or(message.clone(), str::to_string),
        ),
        other => other,
    }
}

/// The TOTAL members every collection inside `value` carries, summed — the stricter of
/// the two bounds a value could be held to, so neither one wide collection nor many
/// narrow ones escapes [`MAX_COLLECTION_MEMBERS`].
///
/// The walk is ITERATIVE, over an explicit stack, for the reason
/// [`attribute_value_size`] gives.
fn value_members(value: &AttributeValue) -> usize {
    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(match value {
            AttributeValue::L(values) => {
                pending.extend(values.iter());
                values.len()
            }
            AttributeValue::M(entries) => {
                pending.extend(entries.values());
                entries.len()
            }
            other => set_len(other).unwrap_or(0),
        });
    }
    total
}

/// The members an update has written so far, plus the ones `value` carries — refused
/// past [`MAX_COLLECTION_MEMBERS`] in TOTAL across the update, so a per-value ceiling
/// cannot be multiplied by the action count. Counted before the value is written.
fn check_members_written(
    written: usize,
    name: &str,
    value: &AttributeValue,
) -> Result<usize, RequestError> {
    let total = written.saturating_add(value_members(value));
    if total > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members(name, total));
    }
    Ok(total)
}

/// Whether `item` has room for `value` at `path` — the clamp that PRECEDES the write,
/// so the item an update builds cannot grow past the documented 400 KB even while it is
/// being built, however many actions the update states.
///
/// Reached only while an update is APPLIED, so the refusal is the one the update path
/// states for that quota ([`ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE`]): the item that crossed
/// it is the one the change would leave behind.
fn check_room_for(
    item: &Item,
    path: &DocumentPath,
    value: &AttributeValue,
) -> Result<(), RequestError> {
    let replaced = path.get(item).map_or(0, attribute_value_size);
    let projected = item_size(item)
        .saturating_sub(replaced)
        .saturating_add(attribute_value_size(value));
    if projected > MAX_ITEM_SIZE_BYTES {
        return Err(RequestError::Validation(
            ITEM_UPDATE_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// One operand of a condition: a document path, a literal value, or `size (path)`.
#[derive(Debug, Clone)]
enum Operand {
    Path(DocumentPath),
    Value(AttributeValue),
    Size(DocumentPath),
}

/// One node of a parsed `ConditionExpression`.
#[derive(Debug, Clone)]
enum ConditionNode {
    /// Every one of these must hold (`AND`).
    All(Vec<ConditionNode>),
    /// At least one of these must hold (`OR`).
    Any(Vec<ConditionNode>),
    Not(Box<ConditionNode>),
    Compare(Operand, CompareOp, Operand),
    Between(Operand, Operand, Operand),
    In(Operand, Vec<Operand>),
    /// `attribute_exists` when true, `attribute_not_exists` when false.
    Exists(DocumentPath, bool),
    AttributeType(DocumentPath, Operand),
    BeginsWith(Operand, Operand),
    Contains(Operand, Operand),
}

/// A `ConditionExpression`, parsed once and then evaluated against the stored item.
#[derive(Debug, Clone)]
pub struct ParsedCondition {
    node: ConditionNode,
}

impl ParsedCondition {
    /// Parse one `ConditionExpression`. The grammar is DynamoDB's own: comparisons,
    /// `BETWEEN`, `IN`, the documented functions, combined with `AND`, `OR` and `NOT`
    /// and grouped with parentheses.
    pub fn parse(
        member: &'static str,
        expression: &str,
        substitutions: Substitutions<'_>,
    ) -> Result<Self, RequestError> {
        // A key of either substitution table that is no substitution TOKEN defines nothing
        // this condition could dereference, so that defect is settled before the condition
        // that would dereference one is read ([`Substitutions::check_keys_are_tokens`]).
        substitutions.check_keys_are_tokens(&[member])?;
        let read = tokenize_expression_spanned(member, expression)?;
        let mut parser = ExprParser::over(member, &read, substitutions);
        let node = parser.disjunction()?;
        if !parser.at_end() {
            return Err(parser.syntax());
        }
        parser.check_no_redundant_parentheses()?;
        parser.check_distinct_first_operands()?;
        parser.check_no_reserved_name()?;
        Ok(Self { node })
    }

    /// Whether the condition holds for `item` — the stored item, or `None` when the
    /// key holds no item, which is what `attribute_not_exists` is for.
    pub fn evaluate(&self, item: Option<&Item>) -> Result<bool, RequestError> {
        let absent = Item::new();
        evaluate_condition(&self.node, item.unwrap_or(&absent))
    }

    /// How this condition READS each top-level attribute it mentions: every attribute it
    /// reads, each with the kinds of read it makes of it.
    ///
    /// What it is for: a condition that may only speak about a named set of attributes, and
    /// only in certain ways — the search schema of a vector index, whose partitioning
    /// attribute may only be pinned by equality — is held to that from the parsed tree,
    /// rather than guessed from the expression's text.
    pub fn attribute_reads(&self) -> BTreeMap<String, BTreeSet<AttributeRead>> {
        let mut reads = BTreeMap::new();
        collect_condition_attributes(&self.node, &mut reads);
        reads
    }
}

/// How a condition reads one top-level attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AttributeRead {
    /// Pinned to one value by the equality operator: `attr = <value>`, or the same
    /// comparison written the other way round.
    Equality,
    /// Read in any other way: another comparison operator, a range, a function, or a
    /// document path that reaches INSIDE the attribute.
    Other,
}

/// Collect how one condition node reads each top-level attribute into `reads`.
///
/// Recursion follows the parsed tree, whose depth the parser holds at
/// [`MAX_EXPRESSION_DEPTH`], so the number of frames is a constant of this service
/// rather than a function of the request.
fn collect_condition_attributes(
    node: &ConditionNode,
    reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>,
) {
    // An equality between an attribute and a VALUE pins that attribute; every other
    // comparison, and an equality against another attribute, reads it some other way.
    if let ConditionNode::Compare(left, CompareOp::Equal, right) = node {
        if let Some((Operand::Path(path), Operand::Value(_))) = match (left, right) {
            (Operand::Path(_), Operand::Value(_)) => Some((left, right)),
            (Operand::Value(_), Operand::Path(_)) => Some((right, left)),
            _ => None,
        } {
            if let Some(attribute) = path.single_attribute() {
                reads
                    .entry(attribute.to_string())
                    .or_default()
                    .insert(AttributeRead::Equality);
                return;
            }
        }
    }
    match node {
        ConditionNode::All(nodes) | ConditionNode::Any(nodes) => {
            for node in nodes {
                collect_condition_attributes(node, reads);
            }
        }
        ConditionNode::Not(inner) => collect_condition_attributes(inner, reads),
        ConditionNode::Compare(left, _, right)
        | ConditionNode::BeginsWith(left, right)
        | ConditionNode::Contains(left, right) => {
            collect_operand_attributes(left, reads);
            collect_operand_attributes(right, reads);
        }
        ConditionNode::Between(value, low, high) => {
            collect_operand_attributes(value, reads);
            collect_operand_attributes(low, reads);
            collect_operand_attributes(high, reads);
        }
        ConditionNode::In(value, candidates) => {
            collect_operand_attributes(value, reads);
            for candidate in candidates {
                collect_operand_attributes(candidate, reads);
            }
        }
        ConditionNode::Exists(path, _) => {
            read_otherwise(path, reads);
        }
        ConditionNode::AttributeType(path, operand) => {
            read_otherwise(path, reads);
            collect_operand_attributes(operand, reads);
        }
    }
}

/// How one operand reads a top-level attribute, if it reads one at all: a literal value
/// reads none, and anything an operand does other than the equality above is `Other`.
fn collect_operand_attributes(
    operand: &Operand,
    reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>,
) {
    match operand {
        Operand::Path(path) | Operand::Size(path) => read_otherwise(path, reads),
        Operand::Value(_) => {}
    }
}

/// Record that `path`'s top-level attribute is read some way other than being pinned.
fn read_otherwise(path: &DocumentPath, reads: &mut BTreeMap<String, BTreeSet<AttributeRead>>) {
    reads
        .entry(path.attribute().to_string())
        .or_default()
        .insert(AttributeRead::Other);
}

/// Evaluate one condition node.
///
/// Recursion follows the parsed tree, whose depth the parser holds at
/// [`MAX_EXPRESSION_DEPTH`]: `AND` and `OR` chains are flattened into one node each,
/// so a long expression widens the tree rather than deepening it.
fn evaluate_condition(node: &ConditionNode, item: &Item) -> Result<bool, RequestError> {
    match node {
        ConditionNode::All(nodes) => {
            for node in nodes {
                if !evaluate_condition(node, item)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        ConditionNode::Any(nodes) => {
            for node in nodes {
                if evaluate_condition(node, item)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        ConditionNode::Not(inner) => Ok(!evaluate_condition(inner, item)?),
        ConditionNode::Compare(left, operator, right) => {
            let left = resolve_operand(left, item);
            let right = resolve_operand(right, item);
            let (Some(left), Some(right)) = (left.as_ref(), right.as_ref()) else {
                // One side addresses nothing: the item carries no value there, and
                // nothing is NOT EQUAL to a value it does not hold, so the one
                // comparator an unheld operand satisfies is the not-equal one — the
                // attribute need not be present for it, while every other comparator
                // (equality included) needs both values to compare. With NEITHER side
                // held there is no value to be unequal to, so nothing holds.
                return Ok(matches!(operator, CompareOp::NotEqual)
                    && (left.is_some() || right.is_some()));
            };
            Ok(match operator {
                CompareOp::Equal => attribute_values_equal(left, right),
                CompareOp::NotEqual => !attribute_values_equal(left, right),
                CompareOp::Less => compare_values(left, right) == Some(Ordering::Less),
                CompareOp::LessOrEqual => {
                    matches!(
                        compare_values(left, right),
                        Some(Ordering::Less | Ordering::Equal)
                    )
                }
                CompareOp::Greater => compare_values(left, right) == Some(Ordering::Greater),
                CompareOp::GreaterOrEqual => matches!(
                    compare_values(left, right),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
            })
        }
        ConditionNode::Between(value, low, high) => {
            let (Some(value), Some(low), Some(high)) = (
                resolve_operand(value, item),
                resolve_operand(low, item),
                resolve_operand(high, item),
            ) else {
                return Ok(false);
            };
            Ok(matches!(
                compare_values(&value, &low),
                Some(Ordering::Greater | Ordering::Equal)
            ) && matches!(
                compare_values(&value, &high),
                Some(Ordering::Less | Ordering::Equal)
            ))
        }
        ConditionNode::In(value, candidates) => {
            let Some(value) = resolve_operand(value, item) else {
                return Ok(false);
            };
            Ok(candidates.iter().any(|candidate| {
                resolve_operand(candidate, item)
                    .is_some_and(|candidate| attribute_values_equal(&value, &candidate))
            }))
        }
        ConditionNode::Exists(path, expected) => Ok(path.get(item).is_some() == *expected),
        ConditionNode::AttributeType(path, operand) => {
            let Some(AttributeValue::S(wanted)) = resolve_operand(operand, item) else {
                return Err(RequestError::Validation(
                    "Invalid ConditionExpression: the second operand of attribute_type must be a \
                     string naming an attribute type"
                        .to_string(),
                ));
            };
            Ok(path
                .get(item)
                .is_some_and(|value| attribute_value_type(value) == wanted))
        }
        ConditionNode::BeginsWith(value, prefix) => {
            let (Some(value), Some(prefix)) =
                (resolve_operand(value, item), resolve_operand(prefix, item))
            else {
                return Ok(false);
            };
            Ok(begins_with_value(&value, &prefix))
        }
        ConditionNode::Contains(haystack, needle) => {
            let (Some(haystack), Some(needle)) = (
                resolve_operand(haystack, item),
                resolve_operand(needle, item),
            ) else {
                return Ok(false);
            };
            Ok(contains_value(&haystack, &needle))
        }
    }
}

/// The value one condition operand stands for, or `None` when it addresses nothing.
fn resolve_operand(operand: &Operand, item: &Item) -> Option<AttributeValue> {
    match operand {
        Operand::Value(value) => Some(value.clone()),
        Operand::Path(path) => path.get(item).cloned(),
        Operand::Size(path) => path
            .get(item)
            .and_then(size_of_value)
            .map(|size| AttributeValue::N(size.to_string())),
    }
}

/// One expression's own text, with the span of each token in it: what a refusal quotes
/// the expression through.
#[derive(Debug, Clone, Copy)]
pub(super) struct ExpressionSource<'a> {
    characters: &'a [char],
    spans: &'a [(usize, usize)],
}

/// Recursive-descent parser over the tokens of one expression.
///
/// `depth` caps how far the rules that descend may: the nesting is caller-chosen, so
/// without the cap the number of stack frames would be too (and a stack overflow
/// aborts the process rather than failing one request).
pub(crate) struct ExprParser<'a> {
    pub(super) member: &'static str,
    pub(super) tokens: &'a [ExprToken],
    pub(super) at: usize,
    pub(super) depth: usize,
    pub(super) substitutions: Substitutions<'a>,
    /// The first word a name position held that is one of DynamoDB's reserved words,
    /// kept rather than refused on the spot — see [`Self::check_no_reserved_name`].
    reserved: Option<String>,
    /// The first operator or function whose first operand is not distinct from the rest,
    /// as the operator the expression wrote and that operand rendered — kept rather than
    /// refused on the spot, see [`Self::check_distinct_first_operands`].
    non_distinct: Option<(String, String)>,
    /// The expression's own characters and the span of each token in them, when the
    /// caller read the expression through [`tokenize_expression_spanned`]: what lets a
    /// refusal quote the text the request sent.
    source: Option<ExpressionSource<'a>>,
}

impl<'a> ExprParser<'a> {
    pub(crate) fn new(
        member: &'static str,
        tokens: &'a [ExprToken],
        substitutions: Substitutions<'a>,
    ) -> Self {
        Self {
            member,
            tokens,
            at: 0,
            depth: 0,
            substitutions,
            reserved: None,
            non_distinct: None,
            source: None,
        }
    }

    /// The same parser over an expression that was read WITH its text, so its refusals
    /// quote the expression as the request spelled it.
    pub(super) fn over(
        member: &'static str,
        read: &'a TokenizedExpression,
        substitutions: Substitutions<'a>,
    ) -> Self {
        Self {
            source: Some(read.source()),
            ..Self::new(member, &read.tokens, substitutions)
        }
    }

    /// The refusal a reserved word written out as an attribute name earns, raised ONCE
    /// THE EXPRESSION HAS PARSED, and after the two rules above it: a name is the LAST
    /// thing an expression is read for, so a pair of parentheses that groups what is
    /// already grouped and an operator whose operands relate one location to itself are
    /// each the mistake whatever the names inside them are.
    ///
    /// A reserved word is a keyword of the expression language, so in an expression that
    /// does not parse it is not yet an attribute name at all: the parse stops where the
    /// grammar stops admitting tokens, and that is what the refusal states
    /// ([`Self::syntax`]) — `Syntax error; token: "<EOF>", near: "<word>"` for a
    /// condition that is one bare word, reserved or not, because a bare path is not a
    /// condition. Only an expression the grammar accepts has name positions to report a
    /// reserved word in, which is why this is asked after the parse rather than inside it
    /// (`attribute_exists(status)` parses, and earns the reserved-word refusal).
    pub(crate) fn check_no_reserved_name(&self) -> Result<(), RequestError> {
        match &self.reserved {
            Some(word) => Err(reserved_attribute_name(self.member, word)),
            None => Ok(()),
        }
    }

    /// Whether THIS operator or function relates one location to itself, noted for the
    /// post-parse refusal ([`Self::check_distinct_first_operands`]) and answered here.
    ///
    /// Answered where the call is read because the rule is read BEFORE the kind and the
    /// domain the signature declares for the remaining positions: two operands naming one
    /// document path is what the expression's text states whatever those positions would
    /// hold, so such a call is reported as the relation it states rather than as an operand
    /// whose declared kind that very relation made impossible — a path repeated into a
    /// position that takes a literal is not a caller writing the wrong kind of operand,
    /// which is the distinction AWS's two sentences draw.
    fn states_non_distinct(&mut self, operator: &str, first: &Operand, rest: &[&Operand]) -> bool {
        let states = non_distinct_first_operand(first, rest).is_some();
        self.note_non_distinct(operator, first, rest);
        states
    }

    /// Note that one operator or function states a first operand that is not distinct from
    /// its remaining ones ([`non_distinct_first_operand`]), keeping the FIRST such defect
    /// the expression states.
    fn note_non_distinct(&mut self, operator: &str, first: &Operand, rest: &[&Operand]) {
        if self.non_distinct.is_some() {
            return;
        }
        if let Some(path) = non_distinct_first_operand(first, rest) {
            self.non_distinct = Some((operator.to_string(), path.render_tokens()));
        }
    }

    /// The refusal an operator or function whose first operand is not distinct from the
    /// remaining ones earns, raised ONCE THE EXPRESSION HAS PARSED.
    ///
    /// Asked after the parentheses and before the names, for the same reason both of those
    /// are asked after the parse: AWS states a rule about an expression's OPERANDS only of
    /// text whose grammar it accepts and whose parentheses say something — an expression
    /// the parse cannot complete is the syntax error it is, and one that groups what is
    /// already grouped is refused for those parentheses whatever its operands then relate.
    pub(crate) fn check_distinct_first_operands(&self) -> Result<(), RequestError> {
        match &self.non_distinct {
            Some((operator, first_operand)) => {
                Err(non_distinct_operands(self.member, operator, first_operand))
            }
            None => Ok(()),
        }
    }

    /// The refusal a REDUNDANT pair of parentheses earns, raised once the expression has
    /// parsed.
    ///
    /// A pair that wraps nothing but another pair groups what is already grouped, so it
    /// states nothing about how the expression is read — and DynamoDB refuses it rather
    /// than reading past it. Asked after the parse, like the reserved-word check above:
    /// parentheses the grammar could not complete are the syntax error the parse stopped
    /// at, and only an expression that parsed has groups to call redundant.
    ///
    /// Grouping is what parentheses do in BOTH grammars this parser reads, so this is asked
    /// where each of them is parsed — the condition and filter language
    /// ([`ParsedCondition::parse`]) and the update language, whose `SET` groups the value it
    /// assigns ([`ParsedUpdate::parse`]). The parentheses of a function call carry its
    /// arguments and are never redundant, which is why a group around a whole call
    /// (`(attribute_exists(a))`) is not one.
    ///
    /// The rule itself is stated once for every grammar of this service that groups with
    /// parentheses ([`states_redundant_parentheses`]); this is where this grammar's tokens
    /// are read as grouping.
    pub(crate) fn check_no_redundant_parentheses(&self) -> Result<(), RequestError> {
        if states_redundant_parentheses(&self.grouping()) {
            return Err(redundant_parentheses(self.member));
        }
        Ok(())
    }

    /// This grammar's tokens as the rules about GROUPING read them: what opens a group,
    /// what closes one, what carries this grammar's CALLS' arguments, what it compares
    /// with, and everything else ([`GroupToken`]).
    fn grouping(&self) -> Vec<GroupToken> {
        self.tokens
            .iter()
            .enumerate()
            .map(|(at, token)| match token {
                ExprToken::Open if self.opens_a_call(at) => GroupToken::Call,
                ExprToken::Open => GroupToken::Open,
                ExprToken::Close => GroupToken::Close,
                ExprToken::Compare(_) => GroupToken::Comparator,
                ExprToken::Word(word)
                    if word.eq_ignore_ascii_case(BETWEEN_OPERATOR)
                        || word.eq_ignore_ascii_case(IN_OPERATOR) =>
                {
                    GroupToken::Comparator
                }
                _ => GroupToken::Other,
            })
            .collect()
    }

    /// The refusal the position the parse stopped at earns.
    ///
    /// Where the parse stopped at a `(` the grammar has no place for, and the expression
    /// never CLOSES that group, the refusal is stated at the END of the expression
    /// instead: a group-opener standing where no group can start is one token the
    /// expression should not carry, and with it out of the way the expression runs out of
    /// text — which is what is wrong with it. `attribute_exists((` is the syntax error at
    /// its end, while `attribute_exists(pk AND` stops at the `AND` it does carry, because
    /// that token is not a stray opener. A group the expression does close is read as the
    /// group it is, so the parse reports it where it stands.
    pub(crate) fn syntax(&self) -> RequestError {
        let at = if self.stopped_at_a_stray_group_opener() {
            self.tokens.len()
        } else {
            self.at
        };
        expression_syntax_error_at(self.member, &self.stopped_token(at), &self.near_text_at(at))
    }

    /// Whether the parse stopped at a `(` that opens a group the expression never closes.
    fn stopped_at_a_stray_group_opener(&self) -> bool {
        if !matches!(self.tokens.get(self.at), Some(ExprToken::Open)) {
            return false;
        }
        let count = |wanted: &ExprToken| self.tokens.iter().filter(|token| *token == wanted).count();
        count(&ExprToken::Open) > count(&ExprToken::Close)
    }

    /// The token at `at`, as the expression wrote it, or the end-of-input marker when it
    /// is past the last token.
    fn stopped_token(&self, at: usize) -> String {
        self.tokens
            .get(at)
            .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), token_text)
    }

    /// The expression text around the position the parse stopped at: the token before it,
    /// the token itself (there is none past the end) and the token after.
    ///
    /// Quoted from the expression's OWN TEXT where the caller kept it, so the spacing
    /// is the request's: `a == :v` reads back as `== :v` rather than as the two `=`
    /// tokens re-rendered with a space between them. Without the text the window is
    /// rendered from the tokens ([`token_window`]).
    fn near_text_at(&self, at: usize) -> String {
        let Some(ExpressionSource { characters, spans }) = self.source else {
            return token_window(self.tokens, at);
        };
        let Some(last_token) = spans.len().checked_sub(1) else {
            return String::new();
        };
        let first = at.saturating_sub(1).min(last_token);
        let last = (at + 1).min(last_token);
        let start = spans.get(first).map_or(0, |span| span.0);
        let end = spans.get(last).map_or(start, |span| span.1);
        characters
            .get(start..end.max(start))
            .map(|window| window.iter().collect())
            .unwrap_or_default()
    }

    /// The attribute name an `#alias` stands for, with an undefined alias refused by
    /// the expression member it travelled in.
    fn alias_name(&self, alias: &str) -> Result<String, RequestError> {
        self.substitutions
            .name(alias)
            .map_err(|error| invalid_expression_member(self.member, error.message()))
    }

    /// The value a `:placeholder` stands for, refused the same way.
    fn placeholder_value(&self, placeholder: &str) -> Result<AttributeValue, RequestError> {
        self.substitutions
            .value(placeholder)
            .map_err(|error| invalid_expression_member(self.member, error.message()))
    }

    pub(crate) fn at_end(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn peek(&self) -> Option<&ExprToken> {
        self.tokens.get(self.at)
    }

    /// The token the parse is AT, left where it is: a grammar position that cannot
    /// accept it has not consumed it, so its refusal names that token
    /// ([`Self::syntax`]) rather than the one after it. A caller that DOES accept the
    /// token advances past it itself. Running out of tokens is the same syntax error at
    /// the end of the expression.
    fn token_here(&self) -> Result<ExprToken, RequestError> {
        self.peek().cloned().ok_or_else(|| self.syntax())
    }

    /// Consume `token` if that is what comes next.
    pub(crate) fn eat(&mut self, token: &ExprToken) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: &ExprToken) -> Result<(), RequestError> {
        if self.eat(token) {
            Ok(())
        } else {
            Err(self.syntax())
        }
    }

    /// Consume a keyword if that is what comes next. The operator and section keywords
    /// are case-insensitive; the FUNCTION names are not ("These function names are
    /// case-sensitive"), so they are matched exactly, elsewhere.
    fn eat_keyword(&mut self, keyword: &str) -> bool {
        match self.peek() {
            Some(ExprToken::Word(word)) if word.eq_ignore_ascii_case(keyword) => {
                self.at += 1;
                true
            }
            _ => false,
        }
    }

    /// The word a CALL starting at `at` names, if a call starts there: a word with this
    /// grammar's `(` just after it. The one reading of what starts a call, so the cursor's
    /// own call ([`Self::peek_call`]) and the `(` a call opens ([`Self::opens_a_call`])
    /// cannot drift apart.
    fn call_at(&self, at: usize) -> Option<&str> {
        match (self.tokens.get(at), self.tokens.get(at + 1)) {
            (Some(ExprToken::Word(word)), Some(ExprToken::Open)) => Some(word.as_str()),
            _ => None,
        }
    }

    /// Whether the `(` at `at` carries a CALL's arguments rather than opening a group: the
    /// word before it names one of this language's functions ([`function_signature`]).
    fn opens_a_call(&self, at: usize) -> bool {
        at.checked_sub(1)
            .and_then(|before| self.call_at(before))
            .is_some_and(|word| function_signature(word).is_some())
    }

    /// The name of the function call that starts here, if one does.
    fn peek_call(&self) -> Option<String> {
        self.call_at(self.at).map(str::to_string)
    }

    fn descend(&mut self) -> Result<(), RequestError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(RequestError::Validation(expression_clause(
                self.member,
                &format!("an expression may nest at most {MAX_EXPRESSION_DEPTH} levels deep"),
            )));
        }
        Ok(())
    }

    /// `path := (name | #alias) ('.' (name | #alias) | '[' index ']')*`
    ///
    /// The whole path is read before its depth is judged, because the refusal REPORTS the
    /// depth the request wrote ([`too_many_nesting_levels`]): a reader that stopped at the
    /// first step past the ceiling could not say how deep the path went. Reading it costs
    /// nothing unbounded — every step consumes at least one token and the token count is
    /// already capped ([`MAX_EXPRESSION_TOKENS`]) — and the path a caller gets back is
    /// still at most [`MAX_ATTRIBUTE_DEPTH`] steps long, which is what bounds every walk
    /// over one.
    pub(crate) fn path(&mut self) -> Result<DocumentPath, RequestError> {
        let mut steps = vec![PathStep::Attribute(self.path_name()?)];
        loop {
            match self.peek().cloned() {
                Some(ExprToken::Dot) => {
                    self.at += 1;
                    steps.push(PathStep::Attribute(self.path_name()?));
                }
                Some(ExprToken::Index(index)) => {
                    self.at += 1;
                    steps.push(PathStep::Index(index));
                }
                _ => break,
            }
        }
        if steps.len() > MAX_ATTRIBUTE_DEPTH {
            return Err(too_many_nesting_levels(self.member, steps.len()));
        }
        Ok(DocumentPath(steps))
    }

    /// One attribute name of a path, written out or behind an `#alias`.
    fn path_name(&mut self) -> Result<String, RequestError> {
        // A bare word in an expression is an attribute NAME, and a name does not begin
        // with a digit: "the first character must be a-z, A-Z, or 0-9" bounds a stored
        // attribute name, but an expression reaches one that cannot be written plainly
        // through `ExpressionAttributeNames` instead ("To access an attribute whose name
        // conflicts with a DynamoDB reserved word" — that member's doc). So a token that
        // begins with a digit is a numeric LITERAL, which the expression language has no
        // place for — every value it compares against is a `:placeholder` — and the
        // expression stops at it as the syntax error it is, before the name is read.
        if self
            .peek()
            .is_some_and(|token| matches!(token, ExprToken::Word(word) if starts_with_digit(word)))
        {
            return Err(self.syntax());
        }
        // Nor is a word this grammar reads as an OPERATOR a name
        // ([`EXPRESSION_OPERATOR_KEYWORDS`]): the text spells the operator it spells, so a
        // name position holding one is a position the grammar cannot take that token at,
        // and the parse stops there — which is what makes a dangling `AND` report the
        // `AND` the operand should have followed rather than the token after it.
        if self
            .peek()
            .is_some_and(|token| matches!(token, ExprToken::Word(word) if is_expression_operator_keyword(word)))
        {
            return Err(self.syntax());
        }
        match self.token_here()? {
            ExprToken::Word(word) => {
                self.at += 1;
                // Noted, not refused: the refusal is this parser's to raise once the
                // whole expression has parsed ([`Self::check_no_reserved_name`]).
                if is_reserved_word(&word) && self.reserved.is_none() {
                    self.reserved = Some(word.clone());
                }
                Ok(word)
            }
            ExprToken::Alias(alias) => {
                self.at += 1;
                self.alias_name(&alias)
            }
            _ => Err(self.syntax()),
        }
    }

    /// `disjunction := conjunction (OR conjunction)*`
    fn disjunction(&mut self) -> Result<ConditionNode, RequestError> {
        let mut nodes = vec![self.conjunction()?];
        while self.eat_keyword("OR") {
            nodes.push(self.conjunction()?);
        }
        Ok(if nodes.len() == 1 {
            nodes.remove(0)
        } else {
            ConditionNode::Any(nodes)
        })
    }

    /// `conjunction := negation (AND negation)*`
    fn conjunction(&mut self) -> Result<ConditionNode, RequestError> {
        let mut nodes = vec![self.negation()?];
        while self.eat_keyword("AND") {
            nodes.push(self.negation()?);
        }
        Ok(if nodes.len() == 1 {
            nodes.remove(0)
        } else {
            ConditionNode::All(nodes)
        })
    }

    /// `negation := NOT negation | primary`
    fn negation(&mut self) -> Result<ConditionNode, RequestError> {
        if self.eat_keyword("NOT") {
            self.descend()?;
            let inner = self.negation()?;
            self.depth -= 1;
            return Ok(ConditionNode::Not(Box::new(inner)));
        }
        self.condition_primary()
    }

    /// `primary := '(' disjunction ')' | function | comparison`
    fn condition_primary(&mut self) -> Result<ConditionNode, RequestError> {
        // A group the grammar reads as a CONDITION. A group whose closing `)` a
        // comparison's own operator follows groups an OPERAND of that comparison instead
        // ([`Self::group_states_an_operand`]), so it is left for the comparison below.
        if matches!(self.peek(), Some(ExprToken::Open)) && !self.group_states_an_operand() {
            self.at += 1;
            self.descend()?;
            let node = self.disjunction()?;
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(node);
        }
        // `size (path)` is an OPERAND rather than a condition of its own, so it falls
        // through to the comparison below.
        if let Some(name) = self.peek_call().filter(|name| name != SIZE_FUNCTION) {
            let node = self.condition_function(&name)?;
            // A function that states a CONDITION states no value, so a comparator written
            // after its call reads that condition as an OPERAND of a comparison — the
            // position is what is wrong with the call and not its name, so the refusal
            // names the function used that way rather than the comparator the parse
            // stopped at ([`function_used_this_way`]).
            if self.continues_a_comparison() {
                return Err(function_used_this_way(self.member, &name));
            }
            return Ok(node);
        }
        let left = self.comparison_operand()?;
        if self.eat_keyword("BETWEEN") {
            let low = self.comparison_operand()?;
            if !self.eat_keyword("AND") {
                return Err(self.syntax());
            }
            let high = self.comparison_operand()?;
            // `BETWEEN` orders its operands, so each literal one is held to the types
            // DynamoDB orders, and the two bounds to the order the comparator requires —
            // both facts of the REQUEST, settled before any item is read.
            for operand in [&left, &low, &high] {
                check_orderable_operand(self.member, BETWEEN_OPERATOR, operand)?;
            }
            // And the range it states is a range BETWEEN its bounds, so neither bound may
            // be the location it is measured against
            // ([`Self::check_distinct_first_operands`]).
            self.note_non_distinct(BETWEEN_OPERATOR, &left, &[&low, &high]);
            check_between_operands(self.member, &low, &high)?;
            return Ok(ConditionNode::Between(left, low, high));
        }
        if self.eat_keyword("IN") {
            self.expect(&ExprToken::Open)?;
            let mut candidates = vec![self.comparison_operand()?];
            while self.eat(&ExprToken::Comma) {
                candidates.push(self.comparison_operand()?);
            }
            self.expect(&ExprToken::Close)?;
            // "IN — ... The IN operator can have up to 100 comparison values"
            // (`ConditionExpression` comparator and function reference): a longer list is
            // refused as the expression it is, before anything is read, naming how many
            // operands the expression gave it ([`MAX_IN_OPERANDS`]).
            if candidates.len() > MAX_IN_OPERANDS {
                return Err(invalid_expression_member(
                    self.member,
                    &format!(
                        "The IN operator is provided with too many operands; number of \
                         operands: {}",
                        candidates.len()
                    ),
                ));
            }
            // A membership test asks whether the first operand is one of the rest, so the
            // rest may not name the location it reads
            // ([`Self::check_distinct_first_operands`]).
            self.note_non_distinct(
                IN_OPERATOR,
                &left,
                &candidates.iter().collect::<Vec<&Operand>>(),
            );
            return Ok(ConditionNode::In(left, candidates));
        }
        match self.peek().cloned() {
            Some(ExprToken::Compare(operator)) => {
                self.at += 1;
                let right = self.comparison_operand()?;
                if operator.orders() {
                    for operand in [&left, &right] {
                        check_orderable_operand(self.member, operator.as_str(), operand)?;
                    }
                }
                // A comparison relates two operands, so the second may not name the
                // location the first reads ([`Self::check_distinct_first_operands`]).
                self.note_non_distinct(operator.as_str(), &left, &[&right]);
                Ok(ConditionNode::Compare(left, operator, right))
            }
            _ => Err(self.operand_stated_as_a_condition(&left)),
        }
    }

    /// The refusal an OPERAND standing where this grammar states a condition earns.
    ///
    /// An operand read by a CALL is a function used in a way the grammar does not state —
    /// `size (path)` "Returns a number representing an attribute's size", and a number is no
    /// condition — so the refusal names that function and the way it is used, exactly as a
    /// condition function written in an operand position does
    /// ([`Self::call_in_an_operand_position`]). An operand the expression wrote WITHOUT a
    /// call states a value or a location, and what the grammar is missing there is the
    /// comparison that would make it a condition: the position the parse stopped at
    /// ([`Self::syntax`]).
    fn operand_stated_as_a_condition(&self, operand: &Operand) -> RequestError {
        if matches!(operand, Operand::Size(_)) {
            return function_used_this_way(self.member, SIZE_FUNCTION);
        }
        self.syntax()
    }

    /// One of the documented condition functions: `attribute_exists`,
    /// `attribute_not_exists`, `attribute_type`, `begins_with` and `contains`.
    ///
    /// The call is held to its documented SIGNATURE before any of it is read against an
    /// item: the number of operands it carries ([`Self::check_operand_count`]) and the
    /// kind each position takes ([`check_operand_kind`]) are facts of the expression's own
    /// text, so a malformed call is refused naming the rule it broke rather than reported
    /// as the grammar position the parse stopped at.
    ///
    /// Those rules speak about the call the expression WROTE, so each of them is asked only
    /// of a call the expression CLOSED ([`Self::operands_in_call`]) — the one reading of
    /// that property this call is read under; text that ends inside a call is the syntax
    /// error the parse stops at.
    fn condition_function(&mut self, name: &str) -> Result<ConditionNode, RequestError> {
        let Some(signature) = condition_function_signature(name) else {
            // Not a condition of this grammar: either a function it may not call at all,
            // or one whose value is an OPERAND written where a condition belongs.
            return Err(if Grammar::Condition.defines(name) {
                function_used_this_way(self.member, name)
            } else {
                function_outside_its_grammar(self.member, Grammar::Condition, name)
            });
        };
        self.at += 2;
        let stated = self.operands_in_call();
        self.check_operand_count(name, signature.len(), stated)?;
        let closed = stated.is_some();
        let node = match name {
            "attribute_exists" => {
                ConditionNode::Exists(self.function_path(name, closed, Grammar::Condition)?, true)
            }
            "attribute_not_exists" => ConditionNode::Exists(
                self.function_path(name, closed, Grammar::Condition)?,
                false,
            ),
            "attribute_type" => {
                let path = self.function_path(name, closed, Grammar::Condition)?;
                self.expect(&ExprToken::Comma)?;
                let wanted = self.argument(|parser| parser.operand())?;
                // A type test whose second operand reads the very location the first
                // names relates that location to itself, which is the defect whatever
                // the position would otherwise take ([`Self::states_non_distinct`]): the
                // kind this position declares and the closed set of type codes are read
                // only of a call that does not state it.
                let non_distinct =
                    self.states_non_distinct(name, &Operand::Path(path.clone()), &[&wanted]);
                if closed && !non_distinct {
                    if let Some(kind) = signature.get(1) {
                        check_operand_kind(self.member, name, *kind, &wanted)?;
                    }
                    check_attribute_type_name(self.member, &wanted)?;
                }
                ConditionNode::AttributeType(path, wanted)
            }
            "begins_with" => {
                let value = self.function_operand(name, signature, 0, closed)?;
                self.expect(&ExprToken::Comma)?;
                let prefix = self.function_operand(name, signature, 1, closed)?;
                // A prefix read from the very location it is tested against is no prefix
                // test, read on the same footing as the type test above: ahead of the
                // domain this position holds a literal to ([`Self::states_non_distinct`]).
                let non_distinct = self.states_non_distinct(name, &value, &[&prefix]);
                if closed && !non_distinct {
                    // BOTH positions relate one text to another, so an operand whose type
                    // the request settles is held to the types that have a prefix in
                    // either of them, in the order the call writes them
                    // ([`check_begins_with_operand`]).
                    check_begins_with_operand(self.member, &value)?;
                    check_begins_with_operand(self.member, &prefix)?;
                }
                ConditionNode::BeginsWith(value, prefix)
            }
            // The closed set above holds nothing else: `contains`, whose searched operand
            // is read at the kind its signature declares like every other position — a
            // location the item holds, or a collection the REQUEST carried.
            _ => {
                let searched = self.function_operand(name, signature, 0, closed)?;
                self.expect(&ExprToken::Comma)?;
                let needle = self.function_operand(name, signature, 1, closed)?;
                // The one rule beyond the operand kinds: the two operands address
                // different locations ([`Self::check_distinct_first_operands`]).
                self.note_non_distinct(name, &searched, &[&needle]);
                ConditionNode::Contains(searched, needle)
            }
        };
        self.expect(&ExprToken::Close)?;
        Ok(node)
    }

    /// Hold the call the parse has just entered to the number of operands its function
    /// takes.
    ///
    /// A call that carries NO operand at all is not held to this: there is nothing there
    /// to be the wrong number OF. Neither is a call the expression never CLOSES, which
    /// states no operand list at all (`stated` is then `None`, see
    /// [`Self::operands_in_call`]): text that ends inside a call is text the grammar cannot
    /// complete, so it is the syntax error the parse stops at, naming the position the
    /// expression ran out, which is what the parse below reports once the next operand
    /// position is read. The arity sentence is for a call whose operand list the
    /// expression finished and that carries too many or too few operands.
    fn check_operand_count(
        &self,
        function: &str,
        takes: usize,
        stated: Option<usize>,
    ) -> Result<(), RequestError> {
        let Some(supplied) = stated else {
            return Ok(());
        };
        if supplied == takes || supplied == 0 {
            return Ok(());
        }
        Err(incorrect_operand_count(self.member, function, supplied))
    }

    /// How many operands the call the parse has just entered carries: one per `,` at this
    /// call's own bracket depth plus one for the last, and none at all for a call that
    /// closes straight away — or `None` for a call the expression never closes, which
    /// states no operand list at all because the text ran out inside it.
    ///
    /// Read ONCE where a call is entered, and the reading is what every rule about that
    /// call's operands is then asked under — how many it carries
    /// ([`Self::check_operand_count`]) and what kind each position takes
    /// ([`check_operand_kind`], [`requires_document_path`]) are all facts of the call the
    /// expression finished writing. Text that ends inside a call never states that call at
    /// all: it is the syntax error the parse stops at ([`Self::syntax`]), which names the
    /// position the expression ran out instead of pointing a caller at a signature.
    ///
    /// Counted off the TOKENS rather than by parsing them, because the count belongs in
    /// the refusal and a call this parser has already rejected is not read any further.
    /// The walk is bounded by [`MAX_EXPRESSION_TOKENS`], like every other pass over an
    /// expression's tokens, so what one request can ask this for is a constant of this
    /// service rather than a function of its text.
    fn operands_in_call(&self) -> Option<usize> {
        let mut depth = 0usize;
        let mut separators = 0usize;
        let mut written = false;
        let mut closed = false;
        for token in self.tokens.iter().skip(self.at) {
            match token {
                ExprToken::Open => {
                    depth += 1;
                    written = true;
                }
                ExprToken::Close if depth == 0 => {
                    closed = true;
                    break;
                }
                ExprToken::Close => depth -= 1,
                ExprToken::Comma if depth == 0 => separators += 1,
                _ => written = true,
            }
        }
        if !closed {
            return None;
        }
        if !written && separators == 0 {
            return Some(0);
        }
        Some(separators + 1)
    }

    /// One ARGUMENT of a call, read through the parentheses that group it.
    ///
    /// Parentheses group what the grammar reads at the position they stand in, and an
    /// argument position reads an operand, so a pair written around an argument states that
    /// argument. The pair the CALL itself opens is already behind the parse by the time an
    /// argument is read, so every `(` standing here opens a group of its own; a pair that
    /// groups what is already grouped is refused once the whole expression has parsed
    /// ([`Self::check_no_redundant_parentheses`]), and the nesting one argument can state is
    /// bounded like every other ([`Self::descend`]).
    fn argument<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, RequestError>,
    ) -> Result<T, RequestError> {
        let mut groups = 0usize;
        while matches!(self.peek(), Some(ExprToken::Open)) {
            self.at += 1;
            self.descend()?;
            groups += 1;
        }
        let read = read(self)?;
        for _ in 0..groups {
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
        }
        Ok(read)
    }

    /// One operand position the signature declares as a document PATH: a `:placeholder`
    /// written there names no location at all, and neither does a CALL to one of this
    /// language's VALUE functions — both are refused naming the function whose position
    /// needs a path, a rule about the operands of a call the expression `closed`, like the
    /// two below it. Which refusal a CALL earns is the one reading of that question every
    /// operand position of this service is held to ([`Self::call_in_an_operand_position`]),
    /// so the call is told apart from a function this `grammar` may not call at all and
    /// from one that states no value for any position to read.
    ///
    /// A word that NAMES a function and opens no call is an attribute name like any
    /// other ([`Self::peek_call`] reads a call as the word with this grammar's `(` just
    /// after it), so `if_not_exists(if_not_exists, :v)` reads the attribute that word
    /// names.
    fn function_path(
        &mut self,
        function: &str,
        closed: bool,
        grammar: Grammar,
    ) -> Result<DocumentPath, RequestError> {
        self.argument(|parser| {
            if closed {
                if let Some(inner) = parser.peek_call() {
                    return Err(parser.call_in_an_operand_position(
                        grammar,
                        &inner,
                        Some(function),
                    ));
                }
                if matches!(parser.peek(), Some(ExprToken::Placeholder(_))) {
                    return Err(requires_document_path(parser.member, function));
                }
            }
            parser.path()
        })
    }

    /// One operand of a call, held to the kind the signature declares for that position.
    fn function_operand(
        &mut self,
        function: &str,
        signature: &[OperandKind],
        at: usize,
        closed: bool,
    ) -> Result<Operand, RequestError> {
        let operand = self.argument(|parser| parser.operand())?;
        if !closed {
            return Ok(operand);
        }
        if let Some(kind) = signature.get(at) {
            check_operand_kind(self.member, function, *kind, &operand)?;
        }
        Ok(operand)
    }

    /// Whether the group that starts here states an OPERAND of a comparison rather than a
    /// condition, in the one reading of that rule every grammar of this service that groups
    /// with parentheses is held to ([`group_states_an_operand`]); this is where this
    /// grammar's own tokens are read as grouping and as comparing ([`Self::grouping`]).
    fn group_states_an_operand(&self) -> bool {
        group_states_an_operand(&self.grouping(), self.at)
    }

    /// Whether the token the parse is AT continues a COMPARISON: one of this grammar's
    /// comparison operators, or one of its two keyword comparators (`BETWEEN` and `IN`),
    /// which is what tells that what was just read stands in an operand position.
    fn continues_a_comparison(&self) -> bool {
        match self.peek() {
            Some(ExprToken::Compare(_)) => true,
            Some(ExprToken::Word(word)) => {
                word.eq_ignore_ascii_case(BETWEEN_OPERATOR)
                    || word.eq_ignore_ascii_case(IN_OPERATOR)
            }
            _ => false,
        }
    }

    /// One operand of a comparison, which the grammar also reads inside a group:
    /// `comparison_operand := '(' comparison_operand ')' | operand`.
    ///
    /// A pair of parentheses around an operand groups what the comparison reads at that
    /// position, so it states the operand it wraps. An ARGUMENT of a call is an operand
    /// position too, read through whatever pairs group it ([`Self::argument`]) and then held
    /// to the kind its signature declares ([`Self::function_operand`]).
    fn comparison_operand(&mut self) -> Result<Operand, RequestError> {
        if matches!(self.peek(), Some(ExprToken::Open)) {
            self.at += 1;
            self.descend()?;
            let operand = self.comparison_operand()?;
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(operand);
        }
        self.operand()
    }

    /// `operand := :value | size '(' path ')' | path`
    ///
    /// `size` is the one function of the condition grammar whose value is an operand
    /// ("size (path) — Returns a number representing an attribute's size"), so every OTHER
    /// call written here states a function where the grammar reads a value or the location
    /// one is read from, and is refused for that ([`Self::call_in_an_operand_position`])
    /// rather than reported as the token the parse stopped at.
    fn operand(&mut self) -> Result<Operand, RequestError> {
        if let Some(ExprToken::Placeholder(placeholder)) = self.peek().cloned() {
            self.at += 1;
            return Ok(Operand::Value(self.placeholder_value(&placeholder)?));
        }
        if let Some(name) = self.peek_call() {
            if name != SIZE_FUNCTION {
                return Err(self.call_in_an_operand_position(Grammar::Condition, &name, None));
            }
            self.at += 2;
            let stated = self.operands_in_call();
            self.check_operand_count(SIZE_FUNCTION, 1, stated)?;
            let path = self.function_path(SIZE_FUNCTION, stated.is_some(), Grammar::Condition)?;
            self.expect(&ExprToken::Close)?;
            return Ok(Operand::Size(path));
        }
        match self.peek() {
            Some(ExprToken::Word(_) | ExprToken::Alias(_)) => Ok(Operand::Path(self.path()?)),
            _ => Err(self.syntax()),
        }
    }

    /// The refusal a CALL written where `grammar` states an OPERAND earns, decided by three
    /// facts about the function it names, in that order:
    ///
    /// * a function this grammar defines no call of is refused by the CONTEXT it was written
    ///   in, or by its spelling where no grammar of this service defines it
    ///   ([`function_outside_its_grammar`]);
    /// * a function that states a CONDITION rather than a value
    ///   ([`function_states_a_condition`]) fills no operand position of any grammar, so it
    ///   is refused for the way it is used ([`function_used_this_way`]);
    /// * a VALUE function fills an operand but not a document PATH, so one written where a
    ///   path belongs is refused for the path that position needs — `needs_a_path` names
    ///   the function whose position it is — and anywhere else for the way it is used.
    fn call_in_an_operand_position(
        &self,
        grammar: Grammar,
        name: &str,
        needs_a_path: Option<&str>,
    ) -> RequestError {
        if !grammar.defines(name) {
            return function_outside_its_grammar(self.member, grammar, name);
        }
        if function_states_a_condition(name) {
            return function_used_this_way(self.member, name);
        }
        match needs_a_path {
            Some(enclosing) => requires_document_path(self.member, enclosing),
            None => function_used_this_way(self.member, name),
        }
    }

    /// `update := (SET sets | REMOVE removes | ADD adds | DELETE deletes)+`, each
    /// section at most once.
    fn update_actions(&mut self) -> Result<Vec<UpdateAction>, RequestError> {
        const SECTIONS: [&str; 4] = ["SET", "REMOVE", "ADD", "DELETE"];
        let mut actions = Vec::new();
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        while !self.at_end() {
            let section = SECTIONS
                .iter()
                .find(|section| self.eat_keyword(section))
                .copied()
                .ok_or_else(|| self.syntax())?;
            if !seen.insert(section) {
                // AWS's own sentence for a section the expression opens twice: the section
                // keyword as the grammar spells it, quoted, and the grammar it is a section
                // of — stated behind the member the text travelled in and inside the
                // envelope every refusal about that member carries
                // ([`invalid_expression_member`]).
                return Err(invalid_expression_member(
                    self.member,
                    &format!(
                        "The \"{section}\" section can only be used once in an update \
                         expression;"
                    ),
                ));
            }
            loop {
                let path = self.path()?;
                actions.push(match section {
                    "SET" => {
                        self.expect(&ExprToken::Compare(CompareOp::Equal))?;
                        UpdateAction::Set(path, self.set_value()?)
                    }
                    "REMOVE" => UpdateAction::Remove(path),
                    "ADD" => UpdateAction::Add(path, self.action_value()?),
                    _ => UpdateAction::Delete(path, self.action_value()?),
                });
                if !self.eat(&ExprToken::Comma) {
                    break;
                }
            }
        }
        if actions.is_empty() {
            return Err(self.syntax());
        }
        Ok(actions)
    }

    /// The value operand `ADD` and `DELETE` take, which is a `:placeholder` — both
    /// actions take a value, not another of the item's attributes.
    fn action_value(&mut self) -> Result<AttributeValue, RequestError> {
        match self.token_here()? {
            ExprToken::Placeholder(placeholder) => {
                self.at += 1;
                self.placeholder_value(&placeholder)
            }
            _ => Err(self.syntax()),
        }
    }

    /// `set_value := '(' set_value ')' | set_operand (('+' | '-') set_operand)?`
    ///
    /// A group states the WHOLE value, so a value that already states an arithmetic
    /// operation is not an operand of another one: an update computes "PLUS or MINUS to an
    /// attribute" (`UpdateItem$UpdateExpression`), one operation per value, and an operator
    /// written after a group is where the chain the expression spells breaks — which is the
    /// token the refusal names, rather than the parenthesis the clause opened.
    fn set_value(&mut self) -> Result<SetValue, RequestError> {
        if self.eat(&ExprToken::Open) {
            self.descend()?;
            let value = self.set_value()?;
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(value);
        }
        let left = self.set_operand()?;
        if self.eat(&ExprToken::Plus) {
            return Ok(SetValue::Sum(left, self.set_operand()?));
        }
        if self.eat(&ExprToken::Minus) {
            return Ok(SetValue::Difference(left, self.set_operand()?));
        }
        Ok(SetValue::Single(left))
    }

    /// `set_operand := :value | if_not_exists '(' path ',' set_operand ')'
    ///              | list_append '(' set_operand ',' set_operand ')' | path`
    ///
    /// Read at the TOP of a `SET` value, where no function encloses the operand.
    fn set_operand(&mut self) -> Result<SetOperand, RequestError> {
        self.set_operand_of(None)
    }

    /// The same operand, read at the position ONE function's operand list states it in:
    /// `enclosing` names that function, and is `None` where the operand is the value's own
    /// (or one side of its arithmetic).
    ///
    /// A function's operand states a value or the location one is read from, so a CALL
    /// written there is a function used where the grammar takes neither
    /// ([`function_nests_in`]) — the one nesting the update-expression reference writes
    /// excepted. Such a call is refused naming the function that is used that way rather
    /// than evaluated, so an expression AWS reads as malformed never writes an item.
    fn set_operand_of(&mut self, enclosing: Option<&str>) -> Result<SetOperand, RequestError> {
        if let Some(name) = self.peek_call() {
            // The two functions a `SET` value may call, each held to its documented
            // signature like every other call this service reads. A call to a function the
            // UPDATE grammar does not define is refused by the context when the expression
            // languages define it elsewhere and by its spelling when they do not
            // ([`function_outside_its_grammar`]).
            if !Grammar::Update.defines(&name) {
                return Err(function_outside_its_grammar(
                    self.member,
                    Grammar::Update,
                    &name,
                ));
            }
            if enclosing.is_some_and(|enclosing| !function_nests_in(&name, enclosing)) {
                return Err(function_used_this_way(self.member, &name));
            }
            self.at += 2;
            self.descend()?;
            let stated = self.operands_in_call();
            self.check_operand_count(&name, 2, stated)?;
            let operand = match name.as_str() {
                "if_not_exists" => {
                    let path = self.function_path(&name, stated.is_some(), Grammar::Update)?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::IfNotExists(path, Box::new(self.set_operand_of(Some(&name))?))
                }
                _ => {
                    let first = self.set_operand_of(Some(&name))?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::ListAppend(
                        Box::new(first),
                        Box::new(self.set_operand_of(Some(&name))?),
                    )
                }
            };
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(operand);
        }
        if let Some(ExprToken::Placeholder(placeholder)) = self.peek().cloned() {
            self.at += 1;
            return Ok(SetOperand::Value(self.placeholder_value(&placeholder)?));
        }
        match self.peek() {
            Some(ExprToken::Word(_) | ExprToken::Alias(_)) => Ok(SetOperand::Path(self.path()?)),
            _ => Err(self.syntax()),
        }
    }
}

/// The refusal for a request that states one thing twice: through an expression member
/// and through the legacy member that expression member replaces ("This is a legacy
/// parameter. Use ConditionExpression instead"). The message names both members, the
/// way the service's own refusal does, so the caller knows which pair to drop.
///
/// Whether it is stated inside the `<N> validation error(s) detected: ` envelope follows
/// the MEMBERS the request held in both generations, the same split
/// [`envelopes_expression_refusals`] states ([`conflict_is_enveloped`]).
pub fn expression_conflict(legacy: &str, expression: &str) -> RequestError {
    expression_conflict_between(&[legacy], &[expression])
}

/// The same refusal over every ask the request stated twice: the legacy members whose own
/// replacement the request carried beside them, and every expression member it carried.
///
/// The rule is about the ASK, not about one pair of members: a request that states two of
/// its asks in both generations is told both at once — "This is a legacy parameter. Use
/// <the expression member> instead" is said of each legacy member — so a caller sees every
/// member it has to drop rather than one pair at a time.
///
/// Each side is listed by the ASK its members state — a request's projection, its filter,
/// its key condition, its update, its condition — walked in the order the calling
/// operation declares the EXPRESSION members those asks are stated through
/// (`smithy_query(query="operation", operation="<the operation>")`), with the members of
/// one ask kept in the order that operation declares them. So a `Scan` that states its
/// projection and its filter in both generations is refused naming
/// `AttributesToGet, ScanFilter, ConditionalOperator` against
/// `ProjectionExpression, FilterExpression`, and a `Query` that also states its key
/// condition both ways names that ask last on each side. Each side is ordered by
/// [`check_expression_parameter_mix`] before it reaches this renderer, and the order the
/// request sent its members in never reaches either.
///
/// The ENVELOPE follows the two sides' MEMBERS, the one rule this module states for every
/// refusal about a request's two generations of parameters
/// ([`conflict_is_enveloped`]): a contradiction between the members a WRITE states its
/// intent through carries the `<N> validation error(s) detected: ` envelope, and one
/// between the members a READ narrows its result with is stated on its own.
pub fn expression_conflict_between(legacy: &[&str], expression: &[&str]) -> RequestError {
    expression_refusal(
        conflict_is_enveloped(legacy, expression),
        format!(
            "Can not use both expression and non-expression parameters in the same request: \
             Non-expression parameters: {{{}}} Expression parameters: {{{}}}",
            legacy.join(", "),
            expression.join(", ")
        ),
    )
}

/// Whether a refusal naming these two sides is stated inside the
/// `<N> validation error(s) detected: ` envelope: it is exactly when either side names a
/// member a WRITE states its intent through ([`ENVELOPED_EXPRESSION_MEMBERS`],
/// [`ENVELOPED_LEGACY_MEMBERS`]).
///
/// The same split [`envelopes_expression_refusals`] answers for a refusal about one
/// expression member's content, read over a refusal that is about TWO members at once: a
/// write's `UpdateExpression` beside `AttributeUpdates`, or its `ConditionExpression`
/// beside `Expected`, is refused inside the envelope, while a read stating its projection
/// or its filter in both generations is refused with the sentence alone.
fn conflict_is_enveloped(legacy: &[&str], expression: &[&str]) -> bool {
    legacy
        .iter()
        .chain(expression.iter())
        .any(|member| states_write_intent(member))
}

/// Whether `member` is one a WRITE states its intent through — the members whose refusals
/// this service carries inside the validation envelope, stated once for both generations.
///
/// The legacy `AttributeUpdates` map needs no entry of its own: a request is refused for
/// stating one thing twice only when it also carries the member that REPLACED it
/// ([`check_expression_parameter_mix`]), so the `UpdateExpression` beside it already
/// answers for the pair.
fn states_write_intent(member: &str) -> bool {
    ENVELOPED_EXPRESSION_MEMBERS.contains(&member) || ENVELOPED_LEGACY_MEMBERS.contains(&member)
}

/// One EXPRESSION member of a request, as a caller states it: its name and whether the
/// request carried it. Its position in the list a caller passes is the ASK it states, which
/// is what a legacy member points at ([`StatedLegacyParameter`]) and what orders both sides
/// of the refusal ([`check_expression_parameter_mix`]): "This is a legacy parameter. Use
/// `<member>` instead" says which member supersedes a legacy one, so neither side's entry
/// needs to name the other's.
pub type StatedParameter<'m> = (&'m str, bool);

/// One legacy member of a request, as a caller states it ([`stated_legacy`],
/// [`stated_legacy_key`]): its name, whether the request carried it, which ASK it states,
/// and whether what supersedes it is the expression language as a whole or that one ask.
#[derive(Debug, Clone, Copy)]
pub struct StatedLegacyParameter<'m> {
    /// The member's name, which is what the refusal names.
    member: &'m str,
    /// Whether the request carried it.
    sent: bool,
    /// The ask it states: the position, in the expression list passed beside it, of the
    /// member that states the same ask. This is what ORDERS the refusal
    /// ([`check_expression_parameter_mix`]), and two legacy members may state one ask — a
    /// legacy filter map and the operator that combines its entries both state the
    /// request's filter.
    ask: usize,
    /// Whether only the expression member at `ask` supersedes this one, rather than any of
    /// them ([`stated_legacy_key`]).
    superseded_within_its_ask: bool,
}

/// One legacy member the EXPRESSION LANGUAGE supersedes: "This is a legacy parameter. Use
/// `<member>` instead" is said of it, and what it states — a projection, a filter, how the
/// entries of a legacy map combine — is optional narrowing of the request, so a request
/// that states any of its asks in that language has superseded this member and is refused
/// beside any expression member it carries. `ask` is the ask it states, which orders the
/// refusal.
pub fn stated_legacy(member: &str, sent: bool, ask: usize) -> StatedLegacyParameter<'_> {
    StatedLegacyParameter {
        member,
        sent,
        ask,
        superseded_within_its_ask: false,
    }
}

/// The legacy member a request states its KEY CONDITION through, which is not narrowing but
/// the one thing the operation cannot run without — "Either the KeyConditions or
/// KeyConditionExpression parameter must be specified in the request"
/// ([`KEY_CONDITION_REQUIRED_MESSAGE`]).
///
/// The operation resolves that condition from whichever member carries it, so this member
/// is held ONLY against the expression member that replaced it: a request that keys the
/// legacy way and filters or projects the expression way has stated each of its asks once
/// and is served.
pub fn stated_legacy_key(member: &str, sent: bool, ask: usize) -> StatedLegacyParameter<'_> {
    StatedLegacyParameter {
        member,
        sent,
        ask,
        superseded_within_its_ask: true,
    }
}

/// Hold a request's ASKS to ONE generation each: a request that states an ask through a
/// legacy (non-expression) member and states any ask in the EXPRESSION language has
/// superseded that member, and is refused before either side is read.
///
/// What a legacy member is held against is [`stated_legacy`]'s business: the expression
/// language supersedes the members that NARROW a request — a projection, a filter, the
/// operator that combines a legacy map's entries — so any expression member collides with
/// them, while the member a request states its KEY CONDITION through is resolved from
/// whichever generation carries it and collides only with the member that replaced it
/// ([`stated_legacy_key`]). So a `Query` keying the legacy way and filtering the expression
/// way is SERVED, each ask stated once, while a `Scan` projecting the legacy way and
/// filtering the expression way is refused.
/// `tests/all/expression_parameter_generations.rs` pins both halves of that.
///
/// A refused request is told what it may not have sent, by the ASK its members state — a
/// request's projection, its filter, its key condition, its update, its condition — in the
/// order the calling operation declares the EXPRESSION members those asks are stated
/// through ([`StatedLegacyParameter`]): the expression side names every expression member
/// the request carried, in that declared order, and the legacy side names every superseded
/// member, ask by ask, keeping the operation's order inside one ask. So a `Query` that
/// states its filter and its key condition in both generations names
/// `QueryFilter, ConditionalOperator` and then `KeyConditions`, the order those asks are
/// declared in.
pub fn check_expression_parameter_mix(
    legacy: &[StatedLegacyParameter<'_>],
    expression: &[StatedParameter<'_>],
) -> Result<(), RequestError> {
    let carried_expression: Vec<&str> = expression
        .iter()
        .filter(|(_, sent)| *sent)
        .map(|(member, _)| *member)
        .collect();
    if carried_expression.is_empty() {
        return Ok(());
    }
    // The superseded members, walked ask by ask: the sort is stable, so two members of one
    // ask keep the order the operation declares them in.
    let mut superseded: Vec<(usize, &str)> = legacy
        .iter()
        .filter(|stated| {
            stated.sent
                && (!stated.superseded_within_its_ask
                    || expression
                        .get(stated.ask)
                        .is_some_and(|(_, sent)| *sent))
        })
        .map(|stated| (stated.ask, stated.member))
        .collect();
    if superseded.is_empty() {
        return Ok(());
    }
    superseded.sort_by_key(|(ask, _)| *ask);
    let named: Vec<&str> = superseded.into_iter().map(|(_, member)| member).collect();
    Err(expression_conflict_between(&named, &carried_expression))
}

/// How a `ConditionalOperator` sent with no map for it to combine is refused.
///
/// The member says how the ENTRIES of the legacy map beside it are combined — "AND - If
/// all of the conditions evaluate to true" (`ConditionalOperator`) — so a request that
/// sends it alone states how to combine nothing, whichever surface it arrived on. One
/// fixed sentence for all of them, naming both kinds of map the member can combine (a
/// read's filter and a write's `Expected`) rather than the one member the calling
/// operation would have carried.
pub const CONDITIONAL_OPERATOR_WITHOUT_MAP_MESSAGE: &str =
    "ConditionalOperator cannot be used without Filter or Expected";

/// How a `ConditionalOperator` sent with a map holding exactly ONE entry is refused.
///
/// A map with one entry states one condition, and one condition is combined with
/// nothing — "AND - If all of the conditions evaluate to true ... OR - If at least one
/// of the conditions evaluate to true" (`ConditionalOperator`) — so the member has no
/// work to do and the request is refused with its own sentence, DISTINCT from the one a
/// request that sent no map at all earns: a caller can tell "you forgot the map" from
/// "your map needs a second entry". Fixed text, named for both kinds of map the member
/// can combine, like the sentence above.
pub const CONDITIONAL_OPERATOR_NEEDS_TWO_ELEMENTS_MESSAGE: &str =
    "ConditionalOperator can only be used when Filter or Expected has two or more elements";

/// The legacy map a conditional WRITE states its condition through, which is what its
/// refusals name.
pub const EXPECTED_MEMBER: &str = "Expected";

/// The legacy condition members a WRITE states its intent through, whose refusals carry
/// the `<N> validation error(s) detected: ` envelope — the members a READ narrows its
/// result with do not, which is the same split [`envelopes_expression_refusals`] states
/// for the two generations' expression members.
const ENVELOPED_LEGACY_MEMBERS: [&str; 1] = [EXPECTED_MEMBER];

/// Whether a refusal about one entry of the legacy condition map `member` — the members an
/// expectation is stated through, the operands of the comparison it names, or the values it
/// compares against — is stated inside the `<N> validation error(s) detected: ` envelope.
///
/// The ONE place that question is answered for the legacy condition maps, so every refusal
/// an entry of one earns follows the MEMBER the request stated its condition through and
/// not the operation that carried it or the check that happened to notice: the map a WRITE
/// guards its payload with is validated inside the envelope on every write that carries it,
/// and the maps a READ narrows its result with are stated on their own
/// ([`ENVELOPED_LEGACY_MEMBERS`]) — the same split [`envelopes_expression_refusals`] states
/// for the two generations' expression members.
fn envelopes_legacy_refusals(member: &str) -> bool {
    ENVELOPED_LEGACY_MEMBERS.contains(&member)
}

/// Whether a `ConditionalOperator` request fits the map it was sent to combine: the
/// member combines the ENTRIES of that map, so it needs two or more of them, and the
/// sentence a request that does not provide them earns is selected on the count the
/// request carried — none, or exactly one.
///
/// `companion_member` is the map the calling surface would have combined (`ScanFilter`,
/// `QueryFilter`, `Expected`), which decides only whether the sentence is stated inside
/// the validation envelope ([`ENVELOPED_LEGACY_MEMBERS`]); the sentence itself names
/// both kinds of map, as AWS states it, and carries nothing of the request.
pub fn check_conditional_operator_companion(
    companion_member: &str,
    conditional_operator: Option<&str>,
    elements: usize,
) -> Result<(), RequestError> {
    if conditional_operator.is_none() {
        return Ok(());
    }
    let clause = match elements {
        0 => CONDITIONAL_OPERATOR_WITHOUT_MAP_MESSAGE,
        1 => CONDITIONAL_OPERATOR_NEEDS_TWO_ELEMENTS_MESSAGE,
        _ => return Ok(()),
    };
    Err(expression_refusal(
        ENVELOPED_LEGACY_MEMBERS.contains(&companion_member),
        clause.to_string(),
    ))
}

/// The clause a legacy `AttributeValueList` whose members are not all of one
/// `AttributeValue` type earns.
///
/// "AttributeValueList must contain two AttributeValue elements of the same type, either
/// String, Number, or Binary" (`BETWEEN` in the `ComparisonOperator` doc) and
/// "AttributeValueList can contain one or more AttributeValue elements of type String,
/// Number, or Binary" (`IN`): the list holds the operands of ONE comparison, so members of
/// two different types state a comparison the service makes against no single value,
/// whatever the addressed item holds. AWS's sentence is fixed and names neither the
/// attribute, the operator nor the types.
const MIXED_OPERAND_TYPES_CLAUSE: &str =
    "AttributeValues inside AttributeValueList must be of same type";

/// The clause a legacy `BETWEEN` whose two bounds the request stated the wrong way round
/// earns.
///
/// "BETWEEN : Greater than or equal to the first value, and less than or equal to the
/// second value" (`ComparisonOperator`): a lower bound above the upper one selects nothing
/// whatever the item holds, so the RANGE is malformed and the condition is never evaluated
/// — the request's own mistake rather than an unmet condition. The expression spelling of
/// the same comparator states the rule in its own member's words
/// ([`check_between_bounds`]); this is the legacy members' sentence, which echoes neither
/// bound.
const REVERSED_BETWEEN_CLAUSE: &str = "The BETWEEN condition was provided a range where \
     the lower bound is greater than the upper bound";

/// The `Attributes` a write reports, by the `ReturnValues` the request asked for.
///
/// `before` is the item as it stood immediately before the write and `after` as it
/// stands immediately after it (`None` when the write removed it); `changed` is what the
/// write changed. An empty projection is reported as no `Attributes` member at all, the
/// way a write that replaced nothing reports none — which is also the answer for an
/// `UPDATED_*` projection whose every changed path addresses nothing in the item being
/// projected.
pub fn projected_return_values(
    return_values: Option<&str>,
    before: Option<&Item>,
    after: Option<&Item>,
    changed: &UpdateOutcome,
) -> Option<Item> {
    let projected = match return_values {
        Some("ALL_OLD") => before.cloned(),
        Some("ALL_NEW") => after.cloned(),
        Some("UPDATED_OLD") => before.map(|item| changed.projected_paths(item)),
        Some("UPDATED_NEW") => after.map(|item| changed.projected_paths(item)),
        _ => None,
    };
    projected.filter(|item| !item.is_empty())
}

/// Just the named attributes of an item.
///
/// The same rule narrows any read to the attributes a request named — a projection over
/// an index entry, say — so a name the item does not carry reports nothing for it rather
/// than a null.
pub fn project_attributes(item: &Item, names: &BTreeSet<String>) -> Item {
    names
        .iter()
        .filter_map(|name| {
            item.get_key_value(name)
                .map(|(name, value)| (name.clone(), value.clone()))
        })
        .collect()
}
