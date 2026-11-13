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
mod operands;
pub use operands::*;

/// The most any one expression member may carry: "Maximum length of any expression
/// string is 4 KB" ("Service, Account, and Table Quotas"), plus the ceilings on the
/// tokens and the nesting that 4 KB of text can express. All three bound the work one
/// request can ask the parsers below for, so neither the token vector nor the parser's
/// stack use is a function of what a caller sends.
pub const MAX_EXPRESSION_BYTES: usize = 4096;
pub const MAX_EXPRESSION_TOKENS: usize = 1024;
pub const MAX_EXPRESSION_DEPTH: usize = 16;

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

/// The most values one `IN` comparison may list: "The list can contain up to 100
/// values, separated by commas" (the Developer Guide's comparison-operator reference
/// for `IN`).
///
/// One ceiling for the legacy spelling of that comparison, whose `AttributeValueList`
/// has no modelled length, so what one `IN` entry costs per item read is a constant of
/// the service rather than of the request body.
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
fn is_expression_word_char(character: char) -> bool {
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
        format!("Invalid {member}: Expression size has exceeded the maximum allowed size;"),
    )
}

/// One clause of the validation envelope for a refusal the EXPRESSION language raises:
/// AWS states the detail behind the expression member it came from, so a caller reading
/// the message knows which member of the request carried the mistake.
///
/// Whether the clause is wrapped in the `<N> validation error(s) detected: ` envelope
/// follows the MEMBER: the members a WRITE states its intent through are validated
/// inside it, and the members a READ narrows its result with are not — the service
/// states those on their own.
fn invalid_expression_member(member: &str, detail: &str) -> RequestError {
    expression_refusal(
        envelopes_expression_refusals(member),
        format!("Invalid {member}: {detail}"),
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
/// and the text around it is the run it sits in together with the run before it.
///
/// That is the same WINDOW the parser's own refusals state ([`token_window`]) — the
/// token before the offending one and the offending one itself — so one expression
/// earns the same shape of message whether the lexer or the parser stopped on it, and a
/// caller reading `near` sees where in the expression to look rather than one isolated
/// run.
pub fn lexical_syntax_error(member: &str, characters: &[char], at: usize) -> RequestError {
    let at = at.min(characters.len());
    let token = characters
        .get(at)
        .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), |c| c.to_string());
    let run_start = characters[..at]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |index| index + 1);
    let end = characters
        .iter()
        .enumerate()
        .skip(at)
        .find(|(_, c)| c.is_whitespace())
        .map_or(characters.len(), |(index, _)| index);
    let current: String = characters[run_start..end.max(run_start)].iter().collect();
    let near = match characters[..run_start]
        .iter()
        .rposition(|c| !c.is_whitespace())
    {
        Some(last) => {
            let start = characters[..=last]
                .iter()
                .rposition(|c| c.is_whitespace())
                .map_or(0, |index| index + 1);
            let previous: String = characters[start..=last].iter().collect();
            format!("{previous} {current}")
        }
        None => current,
    };
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
fn redundant_parentheses(member: &str) -> RequestError {
    invalid_expression_member(member, "The expression has redundant parentheses;")
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
fn is_reserved_word(word: &str) -> bool {
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
        if tokens.len() >= MAX_EXPRESSION_TOKENS {
            return Err(RequestError::Validation(format!(
                "Invalid {member}: an expression may hold at most {MAX_EXPRESSION_TOKENS} tokens"
            )));
        }
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
                    return Err(lexical_syntax_error(member, &characters, at));
                }
                let digits: String = characters[start..end].iter().collect();
                let index = digits
                    .parse::<usize>()
                    .map_err(|_| lexical_syntax_error(member, &characters, at))?;
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
        return Err(invalid_expression_member(
            member,
            "The expression can not be empty;",
        ));
    }
    Ok(TokenizedExpression {
        tokens,
        characters,
        spans,
    })
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
        self.check_an_expression_was_sent(expressions)?;
        self.check_tables_not_empty()?;
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut values: BTreeSet<String> = BTreeSet::new();
        for (member, expression) in expressions {
            let Some(expression) = expression else {
                continue;
            };
            let Ok(tokens) = tokenize_expression(member, expression) else {
                // An expression this cannot read tells nothing about which tokens it
                // references, and the parser that reads it for real refuses it with the
                // syntax error the member earns — which is the refusal the caller gets.
                return Ok(());
            };
            for token in tokens {
                match token {
                    ExprToken::Alias(alias) => {
                        if self.name(&alias).is_err() {
                            return Err(undefined_name(member, &alias));
                        }
                        names.insert(alias);
                    }
                    ExprToken::Placeholder(placeholder) => {
                        if self.value(&placeholder).is_err() {
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
    /// them was sent. The refusal of one ELEMENT's tables is then stated outside the envelope
    /// like every other refusal about an element
    /// ([`Substitutions::check_all_referenced_in_element`]). The NAMES table is reported
    /// first, which is the order the two tables are checked in throughout.
    fn check_an_expression_was_sent(
        &self,
        expressions: &[(&str, Option<&str>)],
    ) -> Result<(), RequestError> {
        let enveloped = expressions
            .iter()
            .any(|(member, _)| envelopes_expression_refusals(member));
        if defines_a_token(self.names) && !names_an_expression(expressions, |_| true) {
            return Err(only_with_expressions(enveloped, "ExpressionAttributeNames"));
        }
        if defines_a_token(self.values) && !names_an_expression(expressions, substitutes_values) {
            return Err(only_with_expressions(enveloped, "ExpressionAttributeValues"));
        }
        Ok(())
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
    fn check_tables_not_empty(&self) -> Result<(), RequestError> {
        if self.names.is_some_and(HashMap::is_empty) {
            return Err(empty_substitution_table("ExpressionAttributeNames"));
        }
        if self.values.is_some_and(HashMap::is_empty) {
            return Err(empty_substitution_table("ExpressionAttributeValues"));
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
            return Err(RequestError::Validation(format!(
                "ExpressionAttributeNames contains invalid value: Empty attribute name for key \
                 {alias}"
            )));
        }
        Ok(())
    }
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

/// The refusal for a substitution table sent with no entries: AWS's own clause, naming
/// the member, inside the validation envelope every refusal of a member's VALUE carries.
pub(super) fn empty_substitution_table(member: &str) -> RequestError {
    refused(format!("{member} must not be empty"))
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

/// The data type an operand-type refusal names, in the vocabulary that family of
/// messages spells types with: the `AttributeValue` member's own documented type name —
/// "An attribute of type String Set" (`AttributeValue$SS`) is `STRING_SET` — rather than
/// the union member tag a type-MISMATCH clause reports
/// ([`crate::state::attribute_value_type`]).
fn operand_type_name(value: &AttributeValue) -> &'static str {
    match value {
        AttributeValue::S(_) => "STRING",
        AttributeValue::N(_) => "NUMBER",
        AttributeValue::B(_) => "BINARY",
        AttributeValue::Ss(_) => "STRING_SET",
        AttributeValue::Ns(_) => "NUMBER_SET",
        AttributeValue::Bs(_) => "BINARY_SET",
        AttributeValue::M(_) => "MAP",
        AttributeValue::L(_) => "LIST",
        AttributeValue::Null(_) => "NULL",
        AttributeValue::Bool(_) => "BOOL",
    }
}

/// What ONE update operator may be given as its value operand: the operator keyword a
/// refusal names, the name AWS gives the set of types that operator takes, and which
/// values are in it.
///
/// The domain is a property of the OPERATOR, not of the item: an operand outside it
/// cannot be applied whatever the attribute holds, so it is refused naming the operator
/// and that type set ([`ValueOperand::check`]), while an operand INSIDE the domain that
/// does not match the type the item holds is the class refusal
/// ([`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`]) the two set walkers raise.
struct ValueOperand {
    operator: &'static str,
    type_set: &'static str,
    takes: fn(&AttributeValue) -> bool,
}

impl ValueOperand {
    /// Hold one operand to this operator's domain, refusing it with the operator, its
    /// own type and this type set's name when it is outside it.
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
/// as an expression spells `ADD` or `DELETE`, while the legacy `AttributeUpdates` map
/// names its action per attribute instead — so a refusal about one of its values has no
/// operator to state and falls back to the class sentence
/// ([`INCORRECT_UPDATE_OPERAND_TYPE_MESSAGE`]).
fn writes_an_operator(member: &str) -> bool {
    member == UPDATE_EXPRESSION_MEMBER
}

/// The refusal for an update operand of a type its operator cannot take at all: AWS
/// names the OPERATOR the expression wrote, the offending operand's own type name and
/// the name of the type set that operator allows, each read off the request, behind the
/// expression member that carried them.
///
/// Stated for a member that writes an operator ([`writes_an_operator`]); the legacy map
/// has none to name, so its values earn the class sentence above.
fn incorrect_update_operand_type_for(
    member: &str,
    operator: &str,
    type_set: &str,
    value: &AttributeValue,
) -> RequestError {
    if !writes_an_operator(member) {
        return incorrect_update_operand_type();
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
    if attribute_depth_exceeds(value, MAX_ATTRIBUTE_DEPTH) {
        return Err(nesting_levels_exceeded());
    }
    // No ONE value can be larger than the item that would have to hold it, so this is
    // also the bound on the bytes a single substitution can carry into the store: the
    // check PRECEDES every clone of it.
    if attribute_value_size(value) > MAX_ITEM_SIZE_BYTES {
        return Err(RequestError::Validation(
            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    if value_members(value) > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members(name, value_members(value)));
    }
    check_value_rules(value)?;
    Ok(())
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

/// The same limits for a value a STATEMENT carries — a `?` parameter or a literal the
/// statement spells — stated as its own sentence rather than inside the validation
/// envelope.
///
/// The rules are the same ([`check_attribute_value`]); what differs is the FORM the
/// service states them in, which follows the surface the value arrived through: the
/// item API validates its members and reports clauses inside the
/// `<N> validation error(s) detected: ` envelope, while the statement surface answers
/// the clause on its own, the way it answers every other refusal about a statement, and
/// states no separator for a value it has nothing to echo after it.
pub fn check_statement_value(name: &str, value: &AttributeValue) -> Result<(), RequestError> {
    check_attribute_value(name, value)
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
            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

/// The refusal for a collection past [`MAX_COLLECTION_MEMBERS`], whether a request sent
/// it or an update's own `ADD`/`list_append` built it.
fn too_many_members(name: &str, members: usize) -> RequestError {
    RequestError::Validation(format!(
        "Attribute {name} holds {members} members, more than this emulator's maximum of \
         {MAX_COLLECTION_MEMBERS}"
    ))
}

/// A set member's canonical sort key, so that members compare and deduplicate by the
/// VALUE they denote: two `N` spellings of one number share one key.
fn canonical_number(text: &str) -> String {
    NumberKey::parse(text).map_or_else(|| text.to_string(), |number| number.canonical_text())
}

/// Whether two sets hold the same members. A DynamoDB set is unordered, so this is
/// member-wise rather than position-wise.
///
/// Both sides are SORTED by `key` rather than walked pairwise: the member count comes
/// from the request, so a pairwise walk would cost the product of two caller-chosen
/// counts, where sorting costs `n log n`.
fn same_members<'v, T: 'v, K: Ord>(
    left: &'v [T],
    right: &'v [T],
    key: impl Fn(&'v T) -> K,
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let key = &key;
    let mut mine: Vec<K> = left.iter().map(key).collect();
    let mut theirs: Vec<K> = right.iter().map(key).collect();
    mine.sort_unstable();
    theirs.sort_unstable();
    mine == theirs
}

/// How two attribute values order, or `None` when DynamoDB documents no order between
/// them: "For type Number, value comparisons are numeric. String value comparisons for
/// greater than, equals, or less than are based on ASCII character code values. For
/// Binary, DynamoDB treats each byte of the binary data as unsigned when it compares
/// binary values" (the `AttributeValueList` doc) — and nothing for the other types, or
/// across types.
pub(crate) fn compare_values(left: &AttributeValue, right: &AttributeValue) -> Option<Ordering> {
    match (left, right) {
        (AttributeValue::S(mine), AttributeValue::S(theirs)) => Some(mine.cmp(theirs)),
        (AttributeValue::B(mine), AttributeValue::B(theirs)) => Some(mine.cmp(theirs)),
        (AttributeValue::N(mine), AttributeValue::N(theirs)) => {
            Some(NumberKey::parse(mine)?.cmp(&NumberKey::parse(theirs)?))
        }
        _ => None,
    }
}

/// `begins_with (path, operand)` — "true if the attribute specified by path begins
/// with a particular substring", which only a string or binary value can.
fn begins_with_value(value: &AttributeValue, prefix: &AttributeValue) -> bool {
    match (value, prefix) {
        (AttributeValue::S(text), AttributeValue::S(prefix)) => text.starts_with(prefix.as_str()),
        (AttributeValue::B(bytes), AttributeValue::B(prefix)) => bytes.starts_with(prefix),
        _ => false,
    }
}

/// `contains (path, operand)` — "true if the attribute specified by path is: A String
/// that contains a particular substring, [or] A Set that contains a particular
/// element". A list is searched for the element too, the way `CONTAINS` reads it.
///
/// A binary value is a sequence of BYTES, and "DynamoDB treats each byte of the binary
/// data as unsigned when it compares binary values" (`AttributeValueList`), so a `B`
/// haystack contains a `B` needle when the needle's bytes appear in it as a contiguous
/// run — any byte value included, `0x00` among them.
fn contains_value(haystack: &AttributeValue, needle: &AttributeValue) -> bool {
    match (haystack, needle) {
        (AttributeValue::S(text), AttributeValue::S(part)) => text.contains(part.as_str()),
        (AttributeValue::B(bytes), AttributeValue::B(part)) => contains_bytes(bytes, part),
        (AttributeValue::Ss(members), AttributeValue::S(member)) => members.contains(member),
        (AttributeValue::Ns(members), AttributeValue::N(member)) => {
            members.iter().any(|stored| numbers_equal(stored, member))
        }
        (AttributeValue::Bs(members), AttributeValue::B(member)) => members.contains(member),
        (AttributeValue::L(members), needle) => members
            .iter()
            .any(|stored| attribute_values_equal(stored, needle)),
        _ => false,
    }
}

/// Whether `part` appears in `bytes` as a contiguous run of bytes. The empty run is in
/// every sequence, the way the empty string is a substring of every string.
///
/// Both lengths are the CALLER's — the haystack is a stored `B` value and the needle an
/// operand the request carried — so the search is the linear-time one `str::contains`
/// uses for text (`memchr::memmem`), and what one evaluation costs is the SUM of the two
/// lengths rather than their product.
fn contains_bytes(bytes: &[u8], part: &[u8]) -> bool {
    if part.is_empty() {
        return true;
    }
    if part.len() > bytes.len() {
        return false;
    }
    ::memchr::memmem::find(bytes, part).is_some()
}

/// `size (path)` — the length of the value at a path: bytes for a string or binary
/// value, members for a set, elements for a list, entries for a map. Nothing else has
/// a documented size.
fn size_of_value(value: &AttributeValue) -> Option<i64> {
    Some(match value {
        AttributeValue::S(text) => byte_len(text.len()),
        AttributeValue::B(bytes) => byte_len(bytes.len()),
        AttributeValue::Ss(members) => byte_len(members.len()),
        AttributeValue::Ns(members) => byte_len(members.len()),
        AttributeValue::Bs(members) => byte_len(members.len()),
        AttributeValue::L(values) => byte_len(values.len()),
        AttributeValue::M(entries) => byte_len(entries.len()),
        AttributeValue::N(_) | AttributeValue::Bool(_) | AttributeValue::Null(_) => return None,
    })
}

/// Add or subtract two `N` values, reporting the canonical text of the result.
///
/// The arithmetic is exact decimal arithmetic (the `bigdecimal` crate), rounded to the
/// documented 38 significant digits, so `SET n = n + :v` and `ADD n :v` answer what
/// DynamoDB answers rather than what binary floating point would. The result is
/// rendered through [`canonical_number_text`], the same canonicalization a written
/// number goes through, so a computed number is refused by the rule it breaks and
/// stored in the one form every read reports.
///
/// The rule a COMPUTED number breaks is stated on its own ([`unenveloped`]): no input
/// member carried that number — the expression asked for it — so there is no member for
/// the validation envelope to report a constraint on.
pub fn number_arithmetic(
    left: &str,
    right: &str,
    subtract: bool,
) -> Result<String, RequestError> {
    let left_value = parse_decimal(left)?;
    let right_value = parse_decimal(right)?;
    let result = if subtract {
        left_value - right_value
    } else {
        left_value + right_value
    };
    let precision = u64::try_from(MAX_SIGNIFICANT_DIGITS).unwrap_or(38);
    let (digits, scale) = result.with_prec(precision).as_bigint_and_exponent();
    // `digits * 10^-scale` is the value, which is exactly what an `N` text spells.
    canonical_number_text(&format!("{digits}e{}", -scale)).map_err(unenveloped)
}

/// One operand of that arithmetic. The canonical text is what reaches the decimal
/// parser, so only one spelling of a number ever does, and a text that denotes no
/// number this store can hold is refused by the rule it breaks.
fn parse_decimal(text: &str) -> Result<BigDecimal, RequestError> {
    let canonical = canonical_number_text(text)?;
    BigDecimal::from_str(&canonical)
        .map_err(|_| number_rejection(text, NumberFault::NotNumeric))
}

/// The members of a set value, as the member-wise set operations below walk them.
enum SetMembers<'a> {
    Strings(&'a [String]),
    Numbers(&'a [String]),
    Binaries(&'a [Vec<u8>]),
}

/// A value's set members, or `None` when it is not a set at all.
fn set_members(value: &AttributeValue) -> Option<SetMembers<'_>> {
    match value {
        AttributeValue::Ss(members) => Some(SetMembers::Strings(members)),
        AttributeValue::Ns(members) => Some(SetMembers::Numbers(members)),
        AttributeValue::Bs(members) => Some(SetMembers::Binaries(members)),
        _ => None,
    }
}

/// How many members a set holds, for the empty-set refusal.
fn set_len(value: &AttributeValue) -> Option<usize> {
    Some(match set_members(value)? {
        SetMembers::Strings(members) | SetMembers::Numbers(members) => members.len(),
        SetMembers::Binaries(members) => members.len(),
    })
}

/// The members a union would hold at most, refused past [`MAX_COLLECTION_MEMBERS`]
/// BEFORE the union is built: a repeated `ADD` must not grow one stored set past the
/// ceiling every value is admitted under.
fn check_union_members(stored: usize, incoming: usize) -> Result<(), RequestError> {
    let total = stored.saturating_add(incoming);
    if total > MAX_COLLECTION_MEMBERS {
        return Err(too_many_members("the set being added to", total));
    }
    Ok(())
}

/// `ADD`'s set form: "If the existing data type is a set and if Value is also a set,
/// then Value is added to the existing set". Both sets must hold the same primitive
/// type, and a member the set already holds is not added twice.
///
/// Membership is tested against a SORTED index of the stored members rather than by
/// scanning them per incoming member: both counts come from the request, so a scan per
/// member would cost their product.
fn set_union(
    existing: &AttributeValue,
    addition: &AttributeValue,
) -> Result<AttributeValue, RequestError> {
    match (existing, addition) {
        (AttributeValue::Ss(stored), AttributeValue::Ss(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<&str> = stored.iter().map(String::as_str).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(member.as_str()) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Ss(members))
        }
        (AttributeValue::Ns(stored), AttributeValue::Ns(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<String> = stored.iter().map(|text| canonical_number(text)).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(canonical_number(member)) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Ns(members))
        }
        (AttributeValue::Bs(stored), AttributeValue::Bs(incoming)) => {
            check_union_members(stored.len(), incoming.len())?;
            let mut held: BTreeSet<&[u8]> = stored.iter().map(Vec::as_slice).collect();
            let mut members = stored.clone();
            for member in incoming {
                if held.insert(member.as_slice()) {
                    members.push(member.clone());
                }
            }
            Ok(AttributeValue::Bs(members))
        }
        (_held, _incoming) => Err(incorrect_update_operand_type()),
    }
}

/// `DELETE`'s set form: "those values are subtracted from the old set". An empty
/// result is no set at all, so the attribute goes away with its last member. The
/// members to remove are indexed first, for the reason [`set_union`] gives.
fn set_difference(
    existing: &AttributeValue,
    removal: &AttributeValue,
) -> Result<Option<AttributeValue>, RequestError> {
    let remaining = match (existing, removal) {
        (AttributeValue::Ss(stored), AttributeValue::Ss(removed)) => {
            let gone: BTreeSet<&str> = removed.iter().map(String::as_str).collect();
            let members: Vec<String> = stored
                .iter()
                .filter(|member| !gone.contains(member.as_str()))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Ss(members))
        }
        (AttributeValue::Ns(stored), AttributeValue::Ns(removed)) => {
            let gone: BTreeSet<String> = removed.iter().map(|text| canonical_number(text)).collect();
            let members: Vec<String> = stored
                .iter()
                .filter(|member| !gone.contains(&canonical_number(member)))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Ns(members))
        }
        (AttributeValue::Bs(stored), AttributeValue::Bs(removed)) => {
            let gone: BTreeSet<&[u8]> = removed.iter().map(Vec::as_slice).collect();
            let members: Vec<Vec<u8>> = stored
                .iter()
                .filter(|member| !gone.contains(member.as_slice()))
                .cloned()
                .collect();
            (!members.is_empty()).then_some(AttributeValue::Bs(members))
        }
        (_held, _removed) => return Err(incorrect_update_operand_type()),
    };
    Ok(remaining)
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
        let read = tokenize_expression_spanned(member, expression)?;
        let mut parser = ExprParser::over(member, &read, substitutions);
        let node = parser.disjunction()?;
        if !parser.at_end() {
            return Err(parser.syntax());
        }
        parser.check_no_reserved_name()?;
        parser.check_no_redundant_parentheses()?;
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

/// One action of a parsed `UpdateExpression` (or of the legacy `AttributeUpdates`
/// map, which states the same four actions a request at a time).
#[derive(Debug, Clone)]
enum UpdateAction {
    Set(DocumentPath, SetValue),
    Remove(DocumentPath),
    Add(DocumentPath, AttributeValue),
    Delete(DocumentPath, AttributeValue),
}

impl UpdateAction {
    /// The path this action writes to.
    fn path(&self) -> &DocumentPath {
        match self {
            Self::Set(path, _) | Self::Remove(path) | Self::Add(path, _) | Self::Delete(path, _) => {
                path
            }
        }
    }
}

/// The right-hand side of one `SET`: one operand, or two joined by `+` or `-`.
#[derive(Debug, Clone)]
enum SetValue {
    Single(SetOperand),
    Sum(SetOperand, SetOperand),
    Difference(SetOperand, SetOperand),
}

/// One operand of a `SET`: a path, a literal value, or one of the two documented
/// functions.
#[derive(Debug, Clone)]
enum SetOperand {
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

/// The update one request states, from `UpdateExpression` or from the legacy
/// `AttributeUpdates` map, parsed once and then applied to the stored item.
#[derive(Debug, Clone)]
pub struct ParsedUpdate {
    /// The input member the update travelled in, which is what every refusal this
    /// update raises names.
    member: &'static str,
    actions: Vec<UpdateAction>,
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
        Self::from_actions("AttributeUpdates", actions)
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
    fn from_actions(
        member: &'static str,
        actions: Vec<UpdateAction>,
    ) -> Result<Self, RequestError> {
        if actions.len() > MAX_UPDATE_ACTIONS {
            return Err(too_many_actions(actions.len()));
        }
        let paths: Vec<&DocumentPath> = actions.iter().map(UpdateAction::path).collect();
        check_paths_disjoint(member, &paths)?;
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
    /// A value an action COMPUTES goes through [`check_attribute_value`] before it is
    /// written, exactly as one a request sent does, and the item is clamped AS IT
    /// GROWS — the room check and the running member count both precede each write —
    /// so a many-action update cannot multiply a per-value bound into an unbounded item
    /// while it is being built, however many actions the expression states.
    pub fn apply(&self, item: &mut Item) -> Result<UpdateOutcome, RequestError> {
        let before = item.clone();
        let mut changed: Vec<DocumentPath> = Vec::new();
        let mut members_written = 0usize;
        let mut removals: Vec<&DocumentPath> = Vec::new();
        for action in &self.actions {
            let path = action.path();
            match action {
                UpdateAction::Set(path, value) => {
                    let value = evaluate_set_value(self.member, value, &before)?;
                    check_attribute_value(path.attribute(), &value)?;
                    members_written =
                        check_members_written(members_written, path.attribute(), &value)?;
                    check_room_for(item, path, &value)?;
                    path.set(item, value)?;
                    changed.push(path.clone());
                }
                UpdateAction::Remove(path) => removals.push(path),
                UpdateAction::Add(_, value) => {
                    let added = apply_add(self.member, path, value, &before)?;
                    check_attribute_value(path.attribute(), &added)?;
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
    changed: Vec<DocumentPath>,
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
    fn projected_paths(&self, item: &Item) -> Item {
        crate::state::project_paths(item, &self.changed)
    }
}

/// The value of an `AttributeValueUpdate` that needs one.
fn required_update_value(
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
fn apply_add(
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
fn apply_delete(
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

/// The value one `SET` writes, evaluated against the item as it stood before the
/// update.
fn evaluate_set_value(
    member: &str,
    value: &SetValue,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    match value {
        SetValue::Single(operand) => evaluate_set_operand(member, operand, before),
        SetValue::Sum(left, right) | SetValue::Difference(left, right) => {
            let subtract = matches!(value, SetValue::Difference(_, _));
            let left = evaluate_set_operand(member, left, before)?;
            let right = evaluate_set_operand(member, right, before)?;
            match (&left, &right) {
                (AttributeValue::N(left), AttributeValue::N(right)) => Ok(AttributeValue::N(
                    number_arithmetic(left, right, subtract)?,
                )),
                // Arithmetic takes two numbers, so the operand that is not one is what
                // the refusal names — the left one when neither is.
                _ => {
                    let offending = if matches!(left, AttributeValue::N(_)) {
                        &right
                    } else {
                        &left
                    };
                    Err(incorrect_operand_type(
                        member,
                        if subtract { "-" } else { "+" },
                        attribute_value_type(offending),
                    ))
                }
            }
        }
    }
}

/// One `SET` operand's value. A path that addresses nothing is the caller's mistake —
/// there is no value to write — rather than a silent no-op.
fn evaluate_set_operand(
    member: &str,
    operand: &SetOperand,
    before: &Item,
) -> Result<AttributeValue, RequestError> {
    match operand {
        SetOperand::Value(value) => Ok(value.clone()),
        SetOperand::Path(path) => path
            .get(before)
            .cloned()
            .ok_or_else(|| RequestError::Validation(MISSING_OPERAND_ATTRIBUTE_MESSAGE.to_string())),
        SetOperand::IfNotExists(path, fallback) => match path.get(before) {
            Some(value) => Ok(value.clone()),
            None => evaluate_set_operand(member, fallback, before),
        },
        SetOperand::ListAppend(first, second) => {
            let first = evaluate_set_operand(member, first, before)?;
            let second = evaluate_set_operand(member, second, before)?;
            match (first, second) {
                (AttributeValue::L(mut head), AttributeValue::L(tail)) => {
                    // The concatenation is bounded BEFORE it is built, and the result
                    // is held to the documented item size: a `list_append` nested
                    // inside another would otherwise double its operand per level, and
                    // only the outermost result ever reaches the item-size check.
                    let total = head.len().saturating_add(tail.len());
                    if total > MAX_COLLECTION_MEMBERS {
                        return Err(too_many_members("the list being appended to", total));
                    }
                    head.extend(tail);
                    let appended = AttributeValue::L(head);
                    if attribute_value_size(&appended) > MAX_ITEM_SIZE_BYTES {
                        return Err(RequestError::Validation(
                            ITEM_SIZE_EXCEEDED_MESSAGE.to_string(),
                        ));
                    }
                    Ok(appended)
                }
                // `list_append` concatenates two lists, so the operand that is not a
                // list is what the refusal names, whichever position it arrived in.
                (first, second) => {
                    let offending = if matches!(first, AttributeValue::L(_)) {
                        &second
                    } else {
                        &first
                    };
                    Err(incorrect_operand_type(
                        member,
                        "list_append",
                        attribute_value_type(offending),
                    ))
                }
            }
        }
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
    /// THE EXPRESSION HAS PARSED.
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

    /// The refusal a REDUNDANT pair of parentheses earns, raised once the expression has
    /// parsed.
    ///
    /// A pair that wraps nothing but another pair groups what is already grouped, so it
    /// states nothing about how the expression is read — and DynamoDB refuses it rather
    /// than reading past it. Asked after the parse, like the reserved-word check above:
    /// parentheses the grammar could not complete are the syntax error the parse stopped
    /// at, and only an expression that parsed has groups to call redundant.
    ///
    /// Grouping is what parentheses do in the CONDITION grammar, so this is asked where
    /// that grammar is parsed ([`ParsedCondition::parse`]) — the parentheses of a function
    /// call carry its arguments and are never redundant, which is why a group around a
    /// whole call (`(attribute_exists(a))`) is not one.
    pub(crate) fn check_no_redundant_parentheses(&self) -> Result<(), RequestError> {
        // The index of the `(` each `)` closes, filled in as the walk closes each group,
        // so the group that ends just before a `)` is known when that `)` is reached.
        let mut opened_by: Vec<Option<usize>> = vec![None; self.tokens.len()];
        let mut open_groups: Vec<usize> = Vec::new();
        for (at, token) in self.tokens.iter().enumerate() {
            match token {
                ExprToken::Open => open_groups.push(at),
                ExprToken::Close => {
                    // A `)` closing nothing is unbalanced, which the grammar states on its
                    // own: there is no group here to call redundant.
                    let Some(open) = open_groups.pop() else {
                        return Ok(());
                    };
                    opened_by[at] = Some(open);
                    // This group's content is exactly one group of its own: it begins where
                    // that group begins and ends where it ends.
                    if at > 0
                        && matches!(self.tokens.get(at - 1), Some(ExprToken::Close))
                        && opened_by[at - 1] == Some(open + 1)
                    {
                        return Err(redundant_parentheses(self.member));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(crate) fn syntax(&self) -> RequestError {
        expression_syntax_error_at(self.member, &self.stopped_token(), &self.near_text())
    }

    /// The token the parse stopped at, as the expression wrote it, or the end-of-input
    /// marker when it ran out of tokens.
    fn stopped_token(&self) -> String {
        self.tokens
            .get(self.at)
            .map_or_else(|| END_OF_EXPRESSION_TOKEN.to_string(), token_text)
    }

    /// The expression text around that token: the token before it, the token itself
    /// (there is none past the end) and the token after.
    ///
    /// Quoted from the expression's OWN TEXT where the caller kept it, so the spacing
    /// is the request's: `a == :v` reads back as `== :v` rather than as the two `=`
    /// tokens re-rendered with a space between them. Without the text the window is
    /// rendered from the tokens ([`token_window`]).
    pub(super) fn near_text(&self) -> String {
        let Some(ExpressionSource { characters, spans }) = self.source else {
            return token_window(self.tokens, self.at);
        };
        let Some(last_token) = spans.len().checked_sub(1) else {
            return String::new();
        };
        let first = self.at.saturating_sub(1).min(last_token);
        let last = (self.at + 1).min(last_token);
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

    fn next_token(&mut self) -> Result<ExprToken, RequestError> {
        let token = self.peek().cloned().ok_or_else(|| self.syntax())?;
        self.at += 1;
        Ok(token)
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

    /// The name of the function call that starts here, if one does.
    fn peek_call(&self) -> Option<String> {
        match (self.peek(), self.tokens.get(self.at + 1)) {
            (Some(ExprToken::Word(word)), Some(ExprToken::Open)) => Some(word.clone()),
            _ => None,
        }
    }

    fn descend(&mut self) -> Result<(), RequestError> {
        self.depth += 1;
        if self.depth > MAX_EXPRESSION_DEPTH {
            return Err(RequestError::Validation(format!(
                "Invalid {}: an expression may nest at most {MAX_EXPRESSION_DEPTH} levels deep",
                self.member
            )));
        }
        Ok(())
    }

    /// `path := (name | #alias) ('.' (name | #alias) | '[' index ']')*`
    pub(crate) fn path(&mut self) -> Result<DocumentPath, RequestError> {
        let mut steps = vec![PathStep::Attribute(self.path_name()?)];
        loop {
            if steps.len() >= MAX_ATTRIBUTE_DEPTH {
                return Err(RequestError::Validation(format!(
                    "Invalid {}: a document path may name at most {MAX_ATTRIBUTE_DEPTH} levels",
                    self.member
                )));
            }
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
        match self.next_token()? {
            ExprToken::Word(word) => {
                // Noted, not refused: the refusal is this parser's to raise once the
                // whole expression has parsed ([`Self::check_no_reserved_name`]).
                if is_reserved_word(&word) && self.reserved.is_none() {
                    self.reserved = Some(word.clone());
                }
                Ok(word)
            }
            ExprToken::Alias(alias) => self.alias_name(&alias),
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
        if self.eat(&ExprToken::Open) {
            self.descend()?;
            let node = self.disjunction()?;
            self.depth -= 1;
            self.expect(&ExprToken::Close)?;
            return Ok(node);
        }
        // `size (path)` is an OPERAND rather than a condition of its own, so it falls
        // through to the comparison below.
        if let Some(name) = self.peek_call().filter(|name| name != "size") {
            return self.condition_function(&name);
        }
        let left = self.operand()?;
        if self.eat_keyword("BETWEEN") {
            let low = self.operand()?;
            if !self.eat_keyword("AND") {
                return Err(self.syntax());
            }
            let high = self.operand()?;
            return Ok(ConditionNode::Between(left, low, high));
        }
        if self.eat_keyword("IN") {
            self.expect(&ExprToken::Open)?;
            let mut candidates = vec![self.operand()?];
            while self.eat(&ExprToken::Comma) {
                candidates.push(self.operand()?);
            }
            self.expect(&ExprToken::Close)?;
            // "IN — ... The IN operator can have up to 100 comparison values"
            // (`ConditionExpression` comparator and function reference), the same
            // ceiling the legacy `IN` comparison is held to ([`MAX_IN_OPERANDS`]): a
            // longer list is refused as the expression it is, before anything is read,
            // naming how many operands the expression gave it.
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
            return Ok(ConditionNode::In(left, candidates));
        }
        match self.next_token()? {
            ExprToken::Compare(operator) => {
                let right = self.operand()?;
                Ok(ConditionNode::Compare(left, operator, right))
            }
            _ => Err(self.syntax()),
        }
    }

    /// One of the documented condition functions: `attribute_exists`,
    /// `attribute_not_exists`, `attribute_type`, `begins_with` and `contains`.
    fn condition_function(&mut self, name: &str) -> Result<ConditionNode, RequestError> {
        self.at += 2;
        let node = match name {
            "attribute_exists" => ConditionNode::Exists(self.path()?, true),
            "attribute_not_exists" => ConditionNode::Exists(self.path()?, false),
            "attribute_type" => {
                let path = self.path()?;
                self.expect(&ExprToken::Comma)?;
                let wanted = self.operand()?;
                check_attribute_type_name(self.member, &wanted)?;
                ConditionNode::AttributeType(path, wanted)
            }
            "begins_with" => {
                let value = self.operand()?;
                self.expect(&ExprToken::Comma)?;
                let prefix = self.operand()?;
                check_begins_with_prefix(self.member, &prefix)?;
                ConditionNode::BeginsWith(value, prefix)
            }
            "contains" => {
                let haystack = self.operand()?;
                self.expect(&ExprToken::Comma)?;
                ConditionNode::Contains(haystack, self.operand()?)
            }
            other => return Err(unknown_function(self.member, other)),
        };
        self.expect(&ExprToken::Close)?;
        Ok(node)
    }

    /// `operand := :value | size '(' path ')' | path`
    fn operand(&mut self) -> Result<Operand, RequestError> {
        if let Some(ExprToken::Placeholder(placeholder)) = self.peek().cloned() {
            self.at += 1;
            return Ok(Operand::Value(self.placeholder_value(&placeholder)?));
        }
        if self.peek_call().as_deref() == Some("size") {
            self.at += 2;
            let path = self.path()?;
            self.expect(&ExprToken::Close)?;
            return Ok(Operand::Size(path));
        }
        match self.peek() {
            Some(ExprToken::Word(_) | ExprToken::Alias(_)) => Ok(Operand::Path(self.path()?)),
            _ => Err(self.syntax()),
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
                return Err(RequestError::Validation(format!(
                    "Invalid {}: the {section} section can only be used once in an expression",
                    self.member
                )));
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
        match self.next_token()? {
            ExprToken::Placeholder(placeholder) => self.placeholder_value(&placeholder),
            _ => Err(self.syntax()),
        }
    }

    /// `set_value := set_operand (('+' | '-') set_operand)?`
    fn set_value(&mut self) -> Result<SetValue, RequestError> {
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
    fn set_operand(&mut self) -> Result<SetOperand, RequestError> {
        if let Some(name) = self.peek_call() {
            self.at += 2;
            self.descend()?;
            let operand = match name.as_str() {
                "if_not_exists" => {
                    let path = self.path()?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::IfNotExists(path, Box::new(self.set_operand()?))
                }
                "list_append" => {
                    let first = self.set_operand()?;
                    self.expect(&ExprToken::Comma)?;
                    SetOperand::ListAppend(Box::new(first), Box::new(self.set_operand()?))
                }
                other => return Err(unknown_function(self.member, other)),
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

/// Hold every VALUE one legacy condition map compares against to the documented value
/// rules, exactly as a value a write stores or an expression binds is held to them.
///
/// The rules belong to the value's TYPE and to this service's own ceilings on the work
/// one value can ask for ([`check_attribute_value`] — the documented nesting depth, the
/// item ceiling, the members a collection may carry, the numeric domain and the set
/// rules), so they hold wherever a request carries a value: an `Expected` entry's
/// `Value` or `AttributeValueList`, and a `ScanFilter`/`QueryFilter` entry's
/// `AttributeValueList`, are values a request carries like any other.
///
/// `member` is the map they arrived in, which decides only whether the clause is stated
/// inside the validation envelope ([`ENVELOPED_LEGACY_MEMBERS`]).
pub fn check_legacy_condition_values<'v>(
    member: &str,
    values: impl Iterator<Item = (&'v str, &'v AttributeValue)>,
) -> Result<(), RequestError> {
    let enveloped = ENVELOPED_LEGACY_MEMBERS.contains(&member);
    for (name, value) in values {
        check_attribute_value(name, value).map_err(|error| {
            if enveloped {
                error
            } else {
                without_envelope(error)
            }
        })?;
    }
    Ok(())
}

/// Hold every value the legacy `Expected` map carries — each entry's `Value` and every
/// element of its `AttributeValueList` — to the documented value rules.
///
/// `Expected` is the THIRD place one write request can carry an attribute value, beside
/// the `Item` it stores and the `ExpressionAttributeValues` its expression binds, and
/// every one of them bounds the work the request can ask for, so the three are held to
/// one bound ([`check_attribute_value`]) before anything walks them. Stated here as the
/// one reader of that map's values, so a handler bounding its own request's values and
/// [`check_write_condition`] cannot read them two different ways.
pub fn check_expected_condition_values(
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
) -> Result<(), RequestError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    check_legacy_condition_values(
        EXPECTED_MEMBER,
        expected.iter().flat_map(|(name, condition)| {
            condition
                .value
                .iter()
                .chain(condition.attribute_value_list.iter().flatten())
                .map(move |value| (name.as_str(), value))
        }),
    )
}

/// Whether the condition a conditional write states is satisfied by the stored item.
///
/// One entry point for all three single-item writes: each states its condition the
/// same two ways — the `ConditionExpression` language, or the legacy `Expected` map
/// combined by `ConditionalOperator` — and they are incompatible with each other
/// ("Use ConditionExpression instead"; a request may not use both).
pub fn write_condition_satisfied(
    condition_expression: Option<&str>,
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
    conditional_operator: Option<&str>,
    substitutions: Substitutions<'_>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    // Every fact of the REQUEST its condition carries is settled first, by the same
    // check the handlers make before they look the table up, so one condition cannot be
    // judged by two different readings of its own members.
    check_write_condition(
        condition_expression,
        expected,
        conditional_operator,
        substitutions,
    )?;
    let expected = expected.filter(|map| !map.is_empty());
    match (condition_expression, expected) {
        (Some(expression), None) => {
            ParsedCondition::parse("ConditionExpression", expression, substitutions)?.evaluate(item)
        }
        (None, Some(expected)) => expected_satisfied(expected, conditional_operator, item),
        // A request stating its condition both ways was refused above, and one stating
        // it neither way asks for nothing to be checked.
        (Some(_), Some(_)) | (None, None) => Ok(true),
    }
}

/// Settle every fact of a conditional write's CONDITION that the request alone
/// decides: which members it stated the condition through, whether the expression
/// parses, whether each `Expected` entry states something checkable, and whether the
/// values it compares against are values this service accepts.
///
/// None of it depends on the table or on the item, so the three single-item writes
/// call it BEFORE they look their table up: a request that cannot be served however
/// the store looks is refused as the malformed request it is, rather than reported as
/// a missing table. [`write_condition_satisfied`] runs the same check, so the two
/// readings of the same members cannot drift apart.
pub fn check_write_condition(
    condition_expression: Option<&str>,
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
    conditional_operator: Option<&str>,
    substitutions: Substitutions<'_>,
) -> Result<(), RequestError> {
    let expected = expected.filter(|map| !map.is_empty());
    match (condition_expression, expected) {
        (Some(_), Some(_)) => Err(expression_conflict("Expected", "ConditionExpression")),
        (Some(expression), None) => {
            if conditional_operator.is_some() {
                return Err(expression_conflict(
                    "ConditionalOperator",
                    "ConditionExpression",
                ));
            }
            ParsedCondition::parse("ConditionExpression", expression, substitutions)?;
            Ok(())
        }
        (None, Some(expected)) => {
            // The member combines the entries of `Expected`, so the map it was sent with
            // has to hold two or more of them.
            check_conditional_operator_companion(
                EXPECTED_MEMBER,
                conditional_operator,
                expected.len(),
            )?;
            // Every value the map compares against is held to the documented value rules,
            // before the stored item is read against any of them.
            check_expected_condition_values(Some(expected))?;
            conditional_operator_any(conditional_operator)?;
            for (name, condition) in expected {
                // Read against NO item: every refusal an entry earns is a fact of the
                // entry, so the walk that evaluates one surfaces exactly those.
                expected_entry_satisfied(name, condition, None)?;
            }
            Ok(())
        }
        (None, None) => check_conditional_operator_companion(EXPECTED_MEMBER, conditional_operator, 0),
    }
}

/// How the entries of a legacy condition map are combined: "AND - If all of the
/// conditions evaluate to true ... OR - If at least one of the conditions evaluate to
/// true" (`ConditionalOperator`), with `AND` the default — answering whether ANY entry
/// holding is enough.
///
/// One reading for every legacy surface that carries the member (`Expected`,
/// `ScanFilter`), so a value outside the model's enum is refused in one wording.
pub fn conditional_operator_any(conditional_operator: Option<&str>) -> Result<bool, RequestError> {
    match conditional_operator {
        None | Some("AND") => Ok(false),
        Some("OR") => Ok(true),
        Some(other) => Err(RequestError::Validation(format!(
            "ConditionalOperator must be AND or OR, got {other}"
        ))),
    }
}

/// Whether the legacy `Expected` map holds for the stored item.
fn expected_satisfied(
    expected: &HashMap<String, ExpectedAttributeValue>,
    conditional_operator: Option<&str>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    let absent = Item::new();
    let item = item.unwrap_or(&absent);
    let mut satisfied = !any;
    for (name, condition) in expected {
        let held = item.get(name);
        let holds = expected_entry_satisfied(name, condition, held)?;
        if any {
            satisfied |= holds;
        } else {
            satisfied &= holds;
        }
    }
    Ok(satisfied)
}

/// One entry of the legacy `Expected` map.
///
/// "Value and Exists are incompatible with AttributeValueList and ComparisonOperator.
/// Note that if you use both sets of parameters at once, DynamoDB will return a
/// ValidationException"; with neither set, there is nothing to check.
fn expected_entry_satisfied(
    name: &str,
    condition: &ExpectedAttributeValue,
    held: Option<&AttributeValue>,
) -> Result<bool, RequestError> {
    let comparison = condition.comparison_operator.as_deref();
    let operands = condition
        .attribute_value_list
        .as_deref()
        .filter(|values| !values.is_empty());
    if (comparison.is_some() || operands.is_some())
        && (condition.value.is_some() || condition.exists.is_some())
    {
        // The two generations of this member are incompatible, and AWS names the pair
        // the request stated its condition through twice: the EXISTENCE form is
        // `Exists` beside `ComparisonOperator`, and the VALUE form is `Value` beside
        // `AttributeValueList`, so the legacy member the request carried decides which
        // pair the sentence names.
        let (legacy, newer) = if condition.exists.is_some() {
            ("Exists", "ComparisonOperator")
        } else {
            ("Value", "AttributeValueList")
        };
        return Err(refused(format!(
            "One or more parameter values were invalid: {legacy} and {newer} cannot be used \
             together for Attribute: {name}"
        )));
    }
    if let Some(operator) = comparison {
        // An operator that compares a value needs one to compare, and an `Expected`
        // entry can carry it in either of two members: AWS names both as what the
        // entry has to state beside the operator it named.
        if operands.is_none() && condition.value.is_none() && takes_operands(operator) {
            return Err(refused(format!(
                "{INVALID_PARAMETER_CLAUSE}: Value or AttributeValueList must be used with \
                 ComparisonOperator: {operator} for Attribute: {name}"
            )));
        }
        return legacy_comparison_matches(operator, held, operands.unwrap_or(&[]));
    }
    match (condition.value.as_ref(), condition.exists) {
        // "If you supply a Value all by itself, DynamoDB assumes the attribute exists"
        // — the attribute must be there AND match.
        (Some(value), None | Some(true)) => {
            Ok(held.is_some_and(|held| attribute_values_equal(held, value)))
        }
        // "Exists is false but you also provide a Value" is refused, as is an `Exists`
        // of true with nothing to check.
        (Some(_), Some(false)) => Err(refused(format!(
            "One or more parameter values were invalid: Value cannot be used when Exists is false \
             for Attribute: {name}"
        ))),
        (None, Some(true)) => Err(refused(format!(
            "One or more parameter values were invalid: Value must be provided when Exists is \
             true for Attribute: {name}"
        ))),
        (None, Some(false)) => Ok(held.is_none()),
        // An entry that states NEITHER member expects nothing: `Exists` defaults to
        // true — "If you supply a Value all by itself, DynamoDB assumes the attribute
        // exists" — and an expectation that the attribute exists needs the value it is
        // expected to hold. AWS names `Exists` as the member that was left out.
        (None, None) => Err(refused(format!(
            "One or more parameter values were invalid: Value must be provided when Exists is \
             null for Attribute: {name}"
        ))),
    }
}

/// Whether one legacy `ComparisonOperator` holds between the stored attribute and the
/// request's `AttributeValueList`, as the `ComparisonOperator` doc defines each one.
pub fn legacy_comparison_matches(
    operator: &str,
    held: Option<&AttributeValue>,
    operands: &[AttributeValue],
) -> Result<bool, RequestError> {
    check_legacy_comparison(operator, operands)?;
    Ok(match operator {
        // "NOT_NULL : the attribute exists"; "NULL : the attribute does not exist".
        "NOT_NULL" => held.is_some(),
        "NULL" => held.is_none(),
        _ => {
            let Some(held) = held else {
                // Every other operator compares a value, and the item holds none here.
                // A comparison needs something to compare, so an item that does not
                // carry the attribute at all satisfies none of them — the one documented
                // exception is `NE`: "NE : Not equal ... If the item contains an
                // AttributeValue of a different type than the one provided in the
                // request, the value does not match" (`ComparisonOperator`), and an item
                // carrying no value for the name is not equal to the one compared
                // against. Every other operator — the negated `NOT_CONTAINS` included —
                // selects only the items that HAVE the attribute, which is what makes a
                // legacy filter answer the item set `attribute_exists` plus the
                // expression form of the comparator answers.
                return Ok(operator == "NE");
            };
            match operator {
                "EQ" => attribute_values_equal(held, &operands[0]),
                "NE" => !attribute_values_equal(held, &operands[0]),
                "LT" => compare_values(held, &operands[0]) == Some(Ordering::Less),
                "LE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
                "GT" => compare_values(held, &operands[0]) == Some(Ordering::Greater),
                "GE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
                "CONTAINS" => contains_value(held, &operands[0]),
                "NOT_CONTAINS" => !contains_value(held, &operands[0]),
                "BEGINS_WITH" => begins_with_value(held, &operands[0]),
                "IN" => operands
                    .iter()
                    .any(|candidate| attribute_values_equal(held, candidate)),
                _ => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    compare_values(held, &operands[1]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
            }
        }
    })
}

/// How many operands one legacy comparison takes, as the `ComparisonOperator` doc
/// defines each one: none for the two existence tests, two for `BETWEEN`, as many as
/// the request chooses for `IN` ([`usize::MAX`] standing for that), and one for every
/// other comparison. `None` for an operator this service does not serve at all.
fn required_operands(operator: &str) -> Option<usize> {
    Some(match operator {
        "NULL" | "NOT_NULL" => 0,
        "BETWEEN" => 2,
        "IN" => usize::MAX,
        "EQ" | "NE" | "LE" | "LT" | "GE" | "GT" | "CONTAINS" | "NOT_CONTAINS" | "BEGINS_WITH" => 1,
        _ => return None,
    })
}

/// Whether a comparison this service serves compares a VALUE, so an entry naming it
/// has to state one — which is what tells an `Expected` entry that named an operator
/// and no value apart from the two existence tests, which state the whole condition in
/// the operator alone.
fn takes_operands(operator: &str) -> bool {
    required_operands(operator).is_some_and(|required| required > 0)
}

/// Whether one legacy comparison is one the service serves at all, and whether it was
/// given the operands its operator takes — the facts of the REQUEST, so a reader that
/// evaluates a legacy condition map against many items settles them once, before it
/// reads anything.
///
/// The operand count is part of each operator's definition in the `ComparisonOperator`
/// doc, so a request that gives the wrong number of them is refused rather than guessed
/// at.
pub fn check_legacy_comparison(
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), RequestError> {
    let Some(required) = required_operands(operator) else {
        return Err(RequestError::Validation(format!(
            "ComparisonOperator must be one of EQ, NE, IN, LE, LT, GE, GT, BETWEEN, NOT_NULL, \
             NULL, CONTAINS, NOT_CONTAINS or BEGINS_WITH, got {operator}"
        )));
    };
    let count_ok = if required == usize::MAX {
        !operands.is_empty()
    } else {
        operands.len() == required
    };
    if !count_ok {
        return Err(RequestError::Validation(invalid_argument_count(operator)));
    }
    // The one operator whose operand count the request chooses, held to the documented
    // ceiling on that list ([`MAX_IN_OPERANDS`]).
    if required == usize::MAX && operands.len() > MAX_IN_OPERANDS {
        return Err(RequestError::Validation(invalid_argument_count(operator)));
    }
    check_legacy_operand_types(operator, operands)?;
    Ok(())
}

/// Hold a legacy comparison's operands to the TYPES that comparison can compare.
///
/// "BEGINS_WITH : Checks for a prefix. ... AttributeValueList can contain only one
/// AttributeValue of type String or Binary (not a Number or a set type)"
/// (`ComparisonOperator`): only those two types have a prefix, so an operand of any
/// other type is refused as the request defect it is — before anything is read, like
/// every other fact of the request. The ordering comparisons are not held here: they
/// compare whatever two values DynamoDB documents an order between, and a type that has
/// none simply does not match.
fn check_legacy_operand_types(
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), RequestError> {
    if operator != "BEGINS_WITH" {
        return Ok(());
    }
    for operand in operands {
        if !matches!(operand, AttributeValue::S(_) | AttributeValue::B(_)) {
            return Err(RequestError::Validation(format!(
                "{INVALID_PARAMETER_CLAUSE}: ComparisonOperator {operator} is not valid for {} \
                 AttributeValue type",
                attribute_value_type(operand)
            )));
        }
    }
    Ok(())
}

/// How a legacy comparison given the wrong number of `AttributeValueList` entries is
/// refused: AWS's sentence for that defect, over the operator the entry named.
///
/// "AttributeValueList - One or more values to evaluate against the supplied attribute.
/// The number of values in the list depends on the ComparisonOperator being used"
/// (`Condition$AttributeValueList`), so the count is part of the operator's own
/// definition — which is why the refusal names the operator and neither the count it
/// takes nor the count that was sent.
fn invalid_argument_count(operator: &str) -> String {
    format!(
        "{INVALID_PARAMETER_CLAUSE}: Invalid number of argument(s) for the {operator} \
         ComparisonOperator"
    )
}

/// Hold a legacy condition MAP to what one request may ask the service to evaluate for
/// every item it reads, and settle every fact of the request its entries carry: how many
/// entries there are ([`MAX_LEGACY_CONDITIONS`]), whether each names a comparison this
/// service serves with the operands that comparison takes
/// ([`check_legacy_comparison`]), and whether the `ConditionalOperator` combining them is
/// one the model declares.
///
/// Settled ONCE, before anything is read, so the work the evaluator below does per item
/// is a function of this service's constants rather than of the request body.
pub fn check_legacy_conditions(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
) -> Result<(), RequestError> {
    if conditions.len() > MAX_LEGACY_CONDITIONS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: a filter may state at most \
             {MAX_LEGACY_CONDITIONS} conditions, got {}",
            conditions.len()
        )));
    }
    for condition in conditions.values() {
        check_legacy_comparison(
            &condition.comparison_operator,
            legacy_operands(condition),
        )?;
    }
    conditional_operator_any(conditional_operator)?;
    Ok(())
}

/// The operands one entry of a legacy condition map compares against.
pub fn legacy_operands(condition: &Condition) -> &[AttributeValue] {
    condition
        .attribute_value_list
        .as_deref()
        .unwrap_or_default()
}

/// Whether a legacy CONDITION MAP — the pre-expression filter surface (`ScanFilter`,
/// `QueryFilter`) — selects `item`, its entries combined by `ConditionalOperator`.
///
/// Each entry is read by the same comparison the legacy conditional writes are read by
/// ([`legacy_comparison_matches`]), which is what makes a `NOT_NULL` or `NULL` entry
/// select exactly the items `attribute_exists` and `attribute_not_exists` select: both
/// forms ask one question of one attribute, through one evaluator.
///
/// The walk stops as soon as the answer is settled — the first entry that fails an `AND`
/// or holds for an `OR` — so what one item costs is bounded by the first decisive entry
/// rather than by the whole map. Whether an entry is one this service can evaluate at all
/// is settled before any item is read ([`check_legacy_conditions`]), so stopping early
/// cannot hide a refusal the request had coming.
pub fn legacy_conditions_match(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
    item: &Item,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    for (name, condition) in conditions {
        let holds = legacy_comparison_matches(
            &condition.comparison_operator,
            item.get(name),
            legacy_operands(condition),
        )?;
        if holds == any {
            return Ok(any);
        }
    }
    Ok(!any)
}

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

