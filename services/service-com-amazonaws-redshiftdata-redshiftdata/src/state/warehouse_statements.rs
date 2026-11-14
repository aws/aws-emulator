// SPDX-License-Identifier: CC0-1.0

//! The statements of the DATA WAREHOUSE's own vocabulary, read from the token
//! stream: `COPY`, `UNLOAD`, `CREATE EXTERNAL SCHEMA`, `SET`, `RESET` and
//! `SHOW`.
//!
//! These are statements of Amazon Redshift rather than of the SQL standard, and
//! the dialect-aware parser this crate reads a statement with models none of
//! them — exactly as it models no `LOCK` — so they are read here from the
//! tokens the dialect's own tokenizer produced ([`super::redshift_sql`] is
//! where that happens, and this module is the grammar it hands them to). A
//! keyword inside a string literal or a comment is part of that literal or
//! comment and starts no statement, because the tokenizer is what decided the
//! tokens.
//!
//! What is read is the FORM of the statement, never what it does: a `COPY`
//! names a source, an authorization and a format, and reading it is what lets
//! the executors behind it ([`super::copy_load`], [`super::unload_export`],
//! [`super::external_schemas`], [`super::session_settings`]) do the work. An
//! option this engine cannot carry out is deliberately NOT read: the statement
//! then goes to the engine as the caller wrote it and the engine reports on it,
//! which is better than reporting a load or an export that did something other
//! than what the option asked for. Each such option is named in the comment
//! beside the ones that are read.
//!
//! Source for the grammars and for every default below: the Amazon Redshift
//! *SQL reference* entries for `COPY`, `UNLOAD`, `CREATE EXTERNAL SCHEMA`,
//! `SET`, `RESET` and `SHOW`.

use sqlparser::tokenizer::Token;

use super::redshift_sql::{is_keyword, qualified_name};

/// One statement of this vocabulary, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WarehouseStatement {
    /// `COPY <table> … FROM <source> …`: load rows into a table from Amazon S3.
    Copy(CopyStatement),
    /// `UNLOAD ('<query>') TO <destination> …`: write a query's rows to Amazon
    /// S3.
    Unload(UnloadStatement),
    /// `CREATE EXTERNAL SCHEMA … FROM DATA CATALOG …`: bind a local schema name
    /// to a database of the AWS Glue Data Catalog.
    CreateExternalSchema(ExternalSchemaStatement),
    /// `CREATE EXTERNAL TABLE <schema>.<table> (…) LOCATION '<s3 uri>'`: define a
    /// table of an external schema, whose data stays in Amazon S3.
    CreateExternalTable(ExternalTableStatement),
    /// `SET <parameter> { TO | = } <value>`: set a parameter for this session.
    Set {
        /// The parameter's name, as the caller spelled it.
        name: String,
        /// The value asked for, or `None` for `DEFAULT` — which is `RESET` of
        /// that one parameter ("SET … DEFAULT … resets the parameter to its
        /// default value").
        value: Option<String>,
    },
    /// `RESET { <parameter> | ALL }`: give a parameter, or every parameter, back
    /// its default.
    Reset {
        /// The parameter to reset, or `None` for `ALL`.
        name: Option<String>,
    },
    /// `SHOW { <parameter> | ALL }`: report the value of a parameter, or of
    /// every parameter.
    Show {
        /// The parameter to report, or `None` for `ALL`.
        name: Option<String>,
    },
}

/// The data format a `COPY` reads or an `UNLOAD` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFormat {
    /// The default character-delimited format — "by default, the COPY command
    /// expects the source data to be character-delimited UTF-8 text", whose
    /// default delimiter is the pipe character.
    Delimited,
    /// `CSV`: comma-separated values, where a field may be quoted and a quote
    /// inside a quoted field is doubled.
    Csv,
    /// `JSON`: one JSON object per record, mapped to the table's columns by
    /// name (`JSON 'auto'`).
    Json,
}

/// `COPY`, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyStatement {
    /// The target table's name parts, as written.
    pub table: Vec<String>,
    /// The columns the statement names, in its order; empty when it names none,
    /// which loads "all the columns in the target table in their default
    /// order".
    pub columns: Vec<String>,
    /// The `FROM` source: an `s3://` prefix, or the key of a manifest when
    /// [`Self::manifest`] is set.
    pub source: String,
    /// The `IAM_ROLE` the load reads the source under, or `None` for a
    /// statement that named another authorization form.
    pub role_arn: Option<String>,
    /// The format the source is read in.
    pub format: DataFormat,
    /// The field delimiter: the format's own default unless `DELIMITER` named
    /// one.
    pub delimiter: char,
    /// The quote character a `CSV` field may be wrapped in (`QUOTE AS`, default
    /// `"`).
    pub quote: char,
    /// `IGNOREHEADER`: how many leading lines of each object are a header and
    /// are not rows.
    pub ignore_header: u64,
    /// `MAXERROR`: how many rows may fail to load before the load itself fails.
    /// "If the load returns the error count, the load fails" — the default is
    /// zero, so one bad row fails the load.
    pub max_error: u64,
    /// `MANIFEST`: the source names a manifest listing the objects to load
    /// rather than the data itself.
    pub manifest: bool,
    /// `NULL AS`: the field value that loads as NULL.
    pub null_as: Option<String>,
    /// `BLANKSASNULL`: a field of only white space loads as NULL.
    pub blanks_as_null: bool,
    /// `EMPTYASNULL`: an empty field loads as NULL.
    pub empty_as_null: bool,
    /// `IGNOREBLANKLINES`: a blank line is not a record.
    pub ignore_blank_lines: bool,
    /// `REMOVEQUOTES`: a delimited field wrapped in quotes loads without them.
    pub remove_quotes: bool,
    /// `FILLRECORD`: a record with fewer fields than the table has columns
    /// loads, the missing trailing columns taking NULL (or empty, for a NOT
    /// NULL column, which this emulator does not declare).
    pub fill_record: bool,
    /// `TRUNCATECOLUMNS`: a character value longer than the column's declared
    /// length is truncated to it rather than failing the row.
    pub truncate_columns: bool,
    /// `NOLOAD`: "checks the validity of the data file without actually loading
    /// the data".
    pub no_load: bool,
    /// `REGION`: the Region the source bucket is in.
    pub region: Option<String>,
}

impl CopyStatement {
    /// A `COPY` of this source into this table, with every option at the
    /// default the statement carries when it names none.
    fn new(table: Vec<String>, columns: Vec<String>, source: String) -> Self {
        Self {
            table,
            columns,
            source,
            role_arn: None,
            format: DataFormat::Delimited,
            delimiter: DEFAULT_DELIMITED_DELIMITER,
            quote: DEFAULT_CSV_QUOTE,
            ignore_header: 0,
            max_error: 0,
            manifest: false,
            null_as: None,
            blanks_as_null: false,
            empty_as_null: false,
            ignore_blank_lines: false,
            remove_quotes: false,
            fill_record: false,
            truncate_columns: false,
            no_load: false,
            region: None,
        }
    }

    /// The delimiter the load really reads fields on: the one `DELIMITER` named,
    /// and otherwise the default of the FORMAT the statement asked for.
    pub fn field_delimiter(&self) -> char {
        if self.delimiter != DEFAULT_DELIMITED_DELIMITER {
            return self.delimiter;
        }
        match self.format {
            DataFormat::Csv => DEFAULT_CSV_DELIMITER,
            DataFormat::Delimited | DataFormat::Json => DEFAULT_DELIMITED_DELIMITER,
        }
    }
}

/// `UNLOAD`, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnloadStatement {
    /// The query whose rows are written, as the caller wrote it inside the
    /// statement's string literal.
    pub query: String,
    /// The `TO` destination: the `s3://bucket/prefix` the objects are written
    /// under, "the name prefix for the set of files".
    pub destination: String,
    /// The `IAM_ROLE` the objects are written under, or `None` for a statement
    /// that named another authorization form.
    pub role_arn: Option<String>,
    /// `MANIFEST`: write a manifest object listing the data objects as well.
    pub manifest: bool,
    /// `HEADER`: write a header line of column names at the top of each object.
    pub header: bool,
    /// The field delimiter written between values.
    pub delimiter: char,
    /// `ADDQUOTES`: wrap every written field in quotes.
    pub add_quotes: bool,
    /// `NULL AS`: the string a NULL value is written as.
    pub null_as: Option<String>,
    /// `ALLOWOVERWRITE`: "By default, UNLOAD fails if it finds files that it
    /// would possibly overwrite" — this option lets it overwrite them.
    pub allow_overwrite: bool,
    /// The format the rows are written in.
    pub format: DataFormat,
    /// `EXTENSION`: the file extension appended to each written object's name.
    pub extension: Option<String>,
    /// `REGION`: the Region the destination bucket is in.
    pub region: Option<String>,
}

impl UnloadStatement {
    /// An `UNLOAD` of this query to this destination, with every option at its
    /// default.
    fn new(query: String, destination: String) -> Self {
        Self {
            query,
            destination,
            role_arn: None,
            manifest: false,
            header: false,
            delimiter: DEFAULT_DELIMITED_DELIMITER,
            add_quotes: false,
            null_as: None,
            allow_overwrite: false,
            format: DataFormat::Delimited,
            extension: None,
            region: None,
        }
    }

    /// The delimiter the export really writes: the one `DELIMITER` named, and
    /// otherwise the default of the format asked for.
    pub fn field_delimiter(&self) -> char {
        if self.delimiter != DEFAULT_DELIMITED_DELIMITER {
            return self.delimiter;
        }
        match self.format {
            DataFormat::Csv => DEFAULT_CSV_DELIMITER,
            DataFormat::Delimited | DataFormat::Json => DEFAULT_DELIMITED_DELIMITER,
        }
    }
}

/// `CREATE EXTERNAL SCHEMA … FROM DATA CATALOG`, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSchemaStatement {
    /// The LOCAL schema name the external database is bound to.
    pub schema: String,
    /// `IF NOT EXISTS`: a schema of that name already bound is not an error.
    pub if_not_exists: bool,
    /// The `DATABASE` the schema is bound to — a database of the AWS Glue Data
    /// Catalog.
    pub catalog_database: String,
    /// The `IAM_ROLE` the catalog and the data are read under, or `None` for
    /// `IAM_ROLE default`.
    pub role_arn: Option<String>,
    /// `CREATE EXTERNAL DATABASE IF NOT EXISTS`: create the catalog database
    /// when it is not there.
    pub create_database: bool,
    /// `REGION`: the Region the catalog is in.
    pub region: Option<String>,
}

/// `CREATE EXTERNAL TABLE`, as the caller wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTableStatement {
    /// The EXTERNAL schema the table belongs to — "the name of the external
    /// table to be created, qualified by an external schema name".
    pub schema: String,
    /// The table's own name.
    pub table: String,
    /// `IF NOT EXISTS`: a table of that name already defined is not an error.
    pub if_not_exists: bool,
    /// Its columns, in the order the statement declares them: each name with the
    /// type as written.
    pub columns: Vec<(String, String)>,
    /// The `PARTITIONED BY` columns, which are columns of the table too.
    pub partitions: Vec<(String, String)>,
    /// `ROW FORMAT DELIMITED FIELDS TERMINATED BY`: the field delimiter its data
    /// is written with.
    pub delimiter: Option<char>,
    /// `ROW FORMAT SERDE '<library>'`: the serialization library its data is
    /// written with.
    pub serde_library: Option<String>,
    /// The `SERDEPROPERTIES` and `TABLE PROPERTIES` the statement carries, in
    /// its order.
    pub properties: Vec<(String, String)>,
    /// `STORED AS <format>`, as written.
    pub stored_as: Option<String>,
    /// `LOCATION`: "the path to the Amazon S3 … folder" the data lives in.
    pub location: String,
}

/// The field delimiter of the default character-delimited format: "the default
/// delimiter is a pipe character ( | )" (`COPY`'s `DELIMITER`).
const DEFAULT_DELIMITED_DELIMITER: char = '|';

/// The field delimiter of the `CSV` format, whose fields are comma-separated.
const DEFAULT_CSV_DELIMITER: char = ',';

/// The quote character a `CSV` field may be wrapped in: "the default quotation
/// mark character is a double quotation mark ( \" )" (`COPY`'s `QUOTE AS`).
const DEFAULT_CSV_QUOTE: char = '"';

/// Read `tokens` as one statement of this vocabulary, or answer `None` for a
/// statement that is not one of them — and for one whose options this engine
/// does not serve, which is then the engine's to report on.
pub fn parse(tokens: &[Token]) -> Option<WarehouseStatement> {
    let head = tokens.first()?;
    if is_keyword(head, "copy") {
        return copy(tokens);
    }
    if is_keyword(head, "unload") {
        return unload(tokens);
    }
    if is_keyword(head, "set") {
        return set(&tokens[1..]);
    }
    if is_keyword(head, "reset") {
        return reset(&tokens[1..]);
    }
    if is_keyword(head, "show") {
        return show(&tokens[1..]);
    }
    if is_keyword(head, "create") {
        return create_external(&tokens[1..]);
    }
    None
}

/// `COPY <table> [ ( <column> [, …] ) ] FROM '<source>' [ <option> … ]`.
fn copy(tokens: &[Token]) -> Option<WarehouseStatement> {
    let (table, rest) = qualified_name(tokens.get(1..)?)?;
    let (columns, rest) = column_list(rest);
    if !is_keyword(rest.first()?, "from") {
        return None;
    }
    let source = literal(rest.get(1)?)?;
    let mut statement = CopyStatement::new(table, columns, source);
    let mut rest = rest.get(2..)?;
    while !rest.is_empty() {
        rest = copy_option(&mut statement, rest)?;
    }
    Some(WarehouseStatement::Copy(statement))
}

/// Read ONE option of a `COPY` and report what follows it. `None` for a token
/// run that is not an option this engine serves, which leaves the whole
/// statement unread.
fn copy_option<'a>(statement: &mut CopyStatement, tokens: &'a [Token]) -> Option<&'a [Token]> {
    let head = tokens.first()?;
    if let Some(rest) = value_clause(tokens, "iam_role") {
        statement.role_arn = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "delimiter") {
        statement.delimiter = one_character(&literal(rest.first()?)?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "quote") {
        statement.quote = one_character(&literal(rest.first()?)?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "null") {
        statement.null_as = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "ignoreheader") {
        statement.ignore_header = count(rest.first()?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "maxerror") {
        // "MAXERROR [AS] error_count", which the documentation also spells
        // `MAXERROR count`; an `ERROR COUNT` noise word is not part of either.
        statement.max_error = count(rest.first()?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "region") {
        statement.region = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "format") {
        let (format, rest) = data_format(rest)?;
        statement.format = format;
        return Some(rest);
    }
    if is_keyword(head, "csv") {
        statement.format = DataFormat::Csv;
        return tokens.get(1..);
    }
    if is_keyword(head, "json") {
        statement.format = DataFormat::Json;
        // `JSON 'auto'` maps each object's fields to the columns of the same
        // name, which is the only mapping served here: a JSONPaths document
        // names a path per column and is not read.
        return match tokens.get(1) {
            Some(token) => match literal(token).as_deref() {
                Some(path) if path.eq_ignore_ascii_case("auto") => tokens.get(2..),
                Some(path) if path.eq_ignore_ascii_case("auto ignorecase") => tokens.get(2..),
                Some(_) => None,
                None => tokens.get(1..),
            },
            None => tokens.get(1..),
        };
    }
    if is_keyword(head, "manifest") {
        statement.manifest = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "blanksasnull") {
        statement.blanks_as_null = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "emptyasnull") {
        statement.empty_as_null = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "ignoreblanklines") {
        statement.ignore_blank_lines = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "removequotes") {
        statement.remove_quotes = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "fillrecord") {
        statement.fill_record = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "truncatecolumns") {
        statement.truncate_columns = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "noload") {
        statement.no_load = true;
        return tokens.get(1..);
    }
    // Options that say something about how the load is PLANNED rather than what
    // it loads: the rows they produce are the same rows either way, so they are
    // read and change nothing.
    if let Some(rest) = switch_clause(tokens, "compupdate") {
        return Some(rest);
    }
    if let Some(rest) = switch_clause(tokens, "statupdate") {
        return Some(rest);
    }
    if let Some(rest) = value_clause(tokens, "comprows") {
        let _rows = count(rest.first()?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "readratio") {
        let _ratio = count(rest.first()?)?;
        return rest.get(1..);
    }
    // Everything else — a compression (`GZIP`, `BZIP2`, `ZSTD`, `LZOP`), a
    // columnar format (`PARQUET`, `ORC`, `AVRO`), `FIXEDWIDTH`, `ENCRYPTED`, an
    // explicit `DATEFORMAT`/`TIMEFORMAT` pattern, `ESCAPE`, `EXPLICIT_IDS` —
    // would change what the load reads or writes, and this engine does not do
    // what they ask: the statement is left unread rather than loaded as if the
    // option had been honoured.
    None
}

/// `UNLOAD ('<query>') TO '<destination>' [ <option> … ]`.
fn unload(tokens: &[Token]) -> Option<WarehouseStatement> {
    let mut rest = tokens.get(1..)?;
    // The query is parenthesised in every documented form of the statement.
    if matches!(rest.first(), Some(Token::LParen)) {
        let query = literal(rest.get(1)?)?;
        if !matches!(rest.get(2), Some(Token::RParen)) {
            return None;
        }
        rest = rest.get(3..)?;
        if !is_keyword(rest.first()?, "to") {
            return None;
        }
        let destination = literal(rest.get(1)?)?;
        let mut statement = UnloadStatement::new(query, destination);
        let mut rest = rest.get(2..)?;
        while !rest.is_empty() {
            rest = unload_option(&mut statement, rest)?;
        }
        return Some(WarehouseStatement::Unload(statement));
    }
    None
}

/// Read ONE option of an `UNLOAD` and report what follows it, on the same terms
/// [`copy_option`] reads a load's options on.
fn unload_option<'a>(statement: &mut UnloadStatement, tokens: &'a [Token]) -> Option<&'a [Token]> {
    let head = tokens.first()?;
    if let Some(rest) = value_clause(tokens, "iam_role") {
        statement.role_arn = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "delimiter") {
        statement.delimiter = one_character(&literal(rest.first()?)?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "null") {
        statement.null_as = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "region") {
        statement.region = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "extension") {
        statement.extension = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "format") {
        let (format, rest) = data_format(rest)?;
        statement.format = format;
        return Some(rest);
    }
    if is_keyword(head, "csv") {
        statement.format = DataFormat::Csv;
        return tokens.get(1..);
    }
    if is_keyword(head, "manifest") {
        statement.manifest = true;
        // "MANIFEST [ VERBOSE ]": the verbose form carries the columns of the
        // unloaded result beside the entries.
        if tokens.get(1).is_some_and(|token| is_keyword(token, "verbose")) {
            return tokens.get(2..);
        }
        return tokens.get(1..);
    }
    if is_keyword(head, "header") {
        statement.header = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "addquotes") {
        statement.add_quotes = true;
        return tokens.get(1..);
    }
    if is_keyword(head, "allowoverwrite") {
        statement.allow_overwrite = true;
        return tokens.get(1..);
    }
    if let Some(rest) = switch_clause(tokens, "parallel") {
        // Every form writes the same rows; how many objects they are spread
        // over is this emulator's own, so the switch changes nothing it says.
        return Some(rest);
    }
    // `CLEANPATH` deletes the objects already at the destination, and this
    // emulator does not delete a caller's objects as a side effect of an
    // export; a compression, a columnar format, `FIXEDWIDTH`, `ESCAPE`,
    // `ENCRYPTED`, `MAXFILESIZE`, `ROWGROUPSIZE` and `PARTITION BY` each change
    // what is written. None of them is read, so a statement naming one is left
    // to the engine rather than exported as if it had been honoured.
    None
}

/// `CREATE EXTERNAL { SCHEMA | TABLE } …`, after the leading `CREATE`.
fn create_external(tokens: &[Token]) -> Option<WarehouseStatement> {
    if !is_keyword(tokens.first()?, "external") {
        return None;
    }
    if is_keyword(tokens.get(1)?, "table") {
        return external_table(tokens.get(2..)?);
    }
    if !is_keyword(tokens.get(1)?, "schema") {
        return None;
    }
    let mut rest = tokens.get(2..)?;
    let if_not_exists = starts_with_keywords(rest, &["if", "not", "exists"]);
    if if_not_exists {
        rest = rest.get(3..)?;
    }
    let schema = name(rest.first()?)?;
    rest = rest.get(1..)?;
    if !is_keyword(rest.first()?, "from") {
        return None;
    }
    rest = rest.get(1..)?;
    // The DATA CATALOG form is the Spectrum one: the external database lives in
    // the AWS Glue Data Catalog. The other sources a Redshift cluster federates
    // to (a Hive metastore, an RDS or Aurora database, another Redshift
    // database) are endpoints this statement would have to reach, so they are
    // not read here.
    if !starts_with_keywords(rest, &["data", "catalog"]) {
        return None;
    }
    rest = rest.get(2..)?;
    let mut statement = ExternalSchemaStatement {
        schema,
        if_not_exists,
        catalog_database: String::new(),
        role_arn: None,
        create_database: false,
        region: None,
    };
    while !rest.is_empty() {
        rest = external_schema_option(&mut statement, rest)?;
    }
    if statement.catalog_database.is_empty() {
        return None;
    }
    Some(WarehouseStatement::CreateExternalSchema(statement))
}

/// Read ONE clause of a `CREATE EXTERNAL SCHEMA` and report what follows it.
fn external_schema_option<'a>(
    statement: &mut ExternalSchemaStatement,
    tokens: &'a [Token],
) -> Option<&'a [Token]> {
    if let Some(rest) = value_clause(tokens, "database") {
        statement.catalog_database = literal(rest.first()?)?;
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "region") {
        statement.region = Some(literal(rest.first()?)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "iam_role") {
        // "IAM_ROLE { default | 'SESSION' | 'arn:aws:iam::<AWS account-id>:role/<role-name>' }"
        let head = rest.first()?;
        if is_keyword(head, "default") {
            return rest.get(1..);
        }
        statement.role_arn = Some(literal(head)?);
        return rest.get(1..);
    }
    if let Some(rest) = value_clause(tokens, "catalog_role") {
        let head = rest.first()?;
        if is_keyword(head, "default") {
            return rest.get(1..);
        }
        let _role = literal(head)?;
        return rest.get(1..);
    }
    if starts_with_keywords(tokens, &["create", "external", "database", "if", "not", "exists"]) {
        statement.create_database = true;
        return tokens.get(6..);
    }
    // `SCHEMA '<schema>'` narrows the external database to one of its schemas,
    // which the Data Catalog has no notion of; `CATALOG_ID`, `URI`, `PORT`,
    // `SECRET_ARN` and the federation clauses each name an endpoint or a
    // catalog this statement would have to reach.
    None
}

/// The STORAGE attributes a `CREATE TABLE` of this dialect may carry, stripped
/// from `sql` — or `None` when it carries none, and for any statement that is
/// not a `CREATE TABLE`.
///
/// "ENCODE encoding", "DISTKEY", "DISTSTYLE", "[COMPOUND | INTERLEAVED]
/// SORTKEY" and "BACKUP { YES | NO }" tell a Redshift cluster how to STORE a
/// table: how each column is compressed, how its rows are distributed over the
/// nodes, in which order they are stored, and whether a snapshot includes them.
/// None of them changes what the table holds or what a query over it answers,
/// and the engine behind this emulator stores a table one way — so a statement
/// that names them creates the table it declares, with the attributes dropped,
/// rather than being refused for a clause about physical layout.
///
/// The text is rebuilt from the TOKENS the dialect's tokenizer produced, with
/// the clause's tokens dropped and every other token kept exactly as it was, so
/// nothing but those clauses is re-spelled.
pub fn without_storage_attributes(sql: &str) -> Option<String> {
    let dialect = sqlparser::dialect::RedshiftSqlDialect {};
    let tokens = sqlparser::tokenizer::Tokenizer::new(&dialect, sql)
        .tokenize()
        .ok()?;
    if !leads_with_create_table(&tokens) {
        return None;
    }
    let mut kept: Vec<Token> = Vec::new();
    let mut index = 0_usize;
    let mut changed = false;
    while index < tokens.len() {
        match storage_attribute(&tokens[index..]) {
            Some(span) if span > 0 => {
                // The white space that led up to the clause goes with it, so the
                // text closes up where the clause was.
                while matches!(kept.last(), Some(Token::Whitespace(_))) {
                    kept.pop();
                }
                index += span;
                changed = true;
            }
            _ => {
                kept.push(tokens[index].clone());
                index += 1;
            }
        }
    }
    changed.then(|| kept.iter().map(ToString::to_string).collect())
}

/// Whether a token stream is a `CREATE [ LOCAL | GLOBAL ] [ TEMP | TEMPORARY ]
/// TABLE` statement, which is the only statement whose storage attributes are
/// stripped.
fn leads_with_create_table(tokens: &[Token]) -> bool {
    let mut significant = tokens
        .iter()
        .filter(|token| !matches!(token, Token::Whitespace(_)));
    if !significant.next().is_some_and(|token| is_keyword(token, "create")) {
        return false;
    }
    for _ in 0..3 {
        match significant.next() {
            Some(token) if is_keyword(token, "table") => return true,
            Some(token)
                if ["local", "global", "temp", "temporary"]
                    .iter()
                    .any(|word| is_keyword(token, word)) => {}
            _ => return false,
        }
    }
    false
}

/// How many tokens the storage attribute at the head of `tokens` spans, or
/// `None` when no such clause starts there.
fn storage_attribute(tokens: &[Token]) -> Option<usize> {
    let head = tokens.first()?;
    // "ENCODE encoding": the compression of one column.
    if is_keyword(head, "encode") {
        return after_word(tokens, 1);
    }
    // "DISTSTYLE { AUTO | EVEN | KEY | ALL }" and "BACKUP { YES | NO }": one
    // word each.
    if is_keyword(head, "diststyle") || is_keyword(head, "backup") {
        return after_word(tokens, 1);
    }
    // "DISTKEY" as a column attribute, and "DISTKEY ( column_name )" as a table
    // attribute.
    if is_keyword(head, "distkey") {
        return Some(after_optional_group(tokens, 1));
    }
    // "SORTKEY" as a column attribute, "SORTKEY AUTO", and "[ COMPOUND |
    // INTERLEAVED ] SORTKEY ( column_name [, …] )".
    if is_keyword(head, "sortkey") {
        let (index, next) = next_significant(tokens, 1)?;
        if next.is_some_and(|token| is_keyword(token, "auto")) {
            return Some(index + 1);
        }
        return Some(after_optional_group(tokens, 1));
    }
    if is_keyword(head, "compound") || is_keyword(head, "interleaved") {
        let (index, next) = next_significant(tokens, 1)?;
        if next.is_some_and(|token| is_keyword(token, "sortkey")) {
            return Some(index + after_optional_group(tokens, index + 1));
        }
        return None;
    }
    None
}

/// The index of the next token from `from` that is not white space, and that
/// token.
fn next_significant(tokens: &[Token], from: usize) -> Option<(usize, Option<&Token>)> {
    let mut index = from;
    while matches!(tokens.get(index), Some(Token::Whitespace(_))) {
        index += 1;
    }
    Some((index, tokens.get(index)))
}

/// How many tokens a clause spans when it is a keyword followed by ONE word.
/// `None` when no word follows, which is not the clause at all.
fn after_word(tokens: &[Token], from: usize) -> Option<usize> {
    let (index, next) = next_significant(tokens, from)?;
    matches!(next, Some(Token::Word(_))).then_some(index + 1)
}

/// How many tokens a clause spans when it is a keyword optionally followed by a
/// parenthesised list.
fn after_optional_group(tokens: &[Token], from: usize) -> usize {
    let Some((index, Some(Token::LParen))) = next_significant(tokens, from) else {
        return from;
    };
    let mut depth = 0_usize;
    let mut position = index;
    while let Some(token) = tokens.get(position) {
        match token {
            Token::LParen => depth += 1,
            Token::RParen => {
                depth -= 1;
                if depth == 0 {
                    return position + 1;
                }
            }
            _ => {}
        }
        position += 1;
    }
    // An unclosed list is not a clause this strips: the statement is left as it
    // stands and the engine reports on it.
    from
}

/// `EXTERNAL TABLE [ IF NOT EXISTS ] <schema>.<table> ( <column> <type> [, …] )
/// [ <clause> … ] LOCATION '<s3 uri>'`, after the leading `CREATE EXTERNAL
/// TABLE`.
fn external_table(tokens: &[Token]) -> Option<WarehouseStatement> {
    let mut rest = tokens;
    let if_not_exists = starts_with_keywords(rest, &["if", "not", "exists"]);
    if if_not_exists {
        rest = rest.get(3..)?;
    }
    let (name, after_name) = qualified_name(rest)?;
    // "the name of the external table to be created, qualified by an external
    // schema name": a name that qualifies nothing names no external schema.
    let [schema, table] = name.as_slice() else {
        return None;
    };
    let (columns, after_columns) = typed_column_list(after_name)?;
    let mut statement = ExternalTableStatement {
        schema: schema.clone(),
        table: table.clone(),
        if_not_exists,
        columns,
        partitions: Vec::new(),
        delimiter: None,
        serde_library: None,
        properties: Vec::new(),
        stored_as: None,
        location: String::new(),
    };
    let mut rest = after_columns;
    while !rest.is_empty() {
        rest = external_table_clause(&mut statement, rest)?;
    }
    // "LOCATION { 's3://bucket/folder/' … }" is not optional: an external table
    // with no location names no data.
    if statement.location.is_empty() {
        return None;
    }
    Some(WarehouseStatement::CreateExternalTable(statement))
}

/// Read ONE clause of a `CREATE EXTERNAL TABLE` and report what follows it.
fn external_table_clause<'a>(
    statement: &mut ExternalTableStatement,
    tokens: &'a [Token],
) -> Option<&'a [Token]> {
    if starts_with_keywords(tokens, &["partitioned", "by"]) {
        let (partitions, rest) = typed_column_list(tokens.get(2..)?)?;
        statement.partitions = partitions;
        return Some(rest);
    }
    if starts_with_keywords(tokens, &["row", "format", "delimited"]) {
        let mut rest = tokens.get(3..)?;
        if starts_with_keywords(rest, &["fields", "terminated", "by"]) {
            statement.delimiter = one_character(&literal(rest.get(3)?)?);
            rest = rest.get(4..)?;
        }
        // The other delimiters this form may name — the escape character, the
        // line terminator, the NULL marker — are the defaults this engine reads
        // a delimited object with, so a statement that names them is read and a
        // statement that names something else for them is not.
        for clause in [
            ["escaped", "by"],
            ["collection", "items"],
            ["map", "keys"],
            ["lines", "terminated"],
            ["null", "defined"],
        ] {
            if starts_with_keywords(rest, &clause) {
                return None;
            }
        }
        return Some(rest);
    }
    if starts_with_keywords(tokens, &["row", "format", "serde"]) {
        statement.serde_library = Some(literal(tokens.get(3)?)?);
        let rest = tokens.get(4..)?;
        if starts_with_keywords(rest, &["with", "serdeproperties"]) {
            let (properties, rest) = property_list(rest.get(2..)?)?;
            statement.properties.extend(properties);
            return Some(rest);
        }
        return Some(rest);
    }
    if let Some(rest) = value_clause(tokens, "location") {
        statement.location = literal(rest.first()?)?;
        return rest.get(1..);
    }
    if starts_with_keywords(tokens, &["stored", "as"]) {
        statement.stored_as = Some(name(tokens.get(2)?)?);
        return tokens.get(3..);
    }
    if starts_with_keywords(tokens, &["table", "properties"]) {
        let (properties, rest) = property_list(tokens.get(2..)?)?;
        statement.properties.extend(properties);
        return Some(rest);
    }
    // `STORED BY` names a storage handler, and `AS <query>` is a create from a
    // query's result, which writes data rather than describing data that is
    // already there.
    None
}

/// A list of `<name>`/`<value>` pairs read from a statement, in the order it
/// writes them: the columns a declaration names with their types, and the
/// properties a clause names with their values.
type NamedValues = Vec<(String, String)>;

/// A parenthesised list of `<name> <type>` declarations, and what follows it.
fn typed_column_list(tokens: &[Token]) -> Option<(NamedValues, &[Token])> {
    if !matches!(tokens.first(), Some(Token::LParen)) {
        return None;
    }
    let mut columns: Vec<(String, String)> = Vec::new();
    let mut rest = tokens.get(1..)?;
    loop {
        let column = name(rest.first()?)?;
        let (declared, tail) = type_text(rest.get(1..)?)?;
        columns.push((column, declared));
        rest = tail;
        match rest.first()? {
            Token::Comma => rest = rest.get(1..)?,
            Token::RParen => return Some((columns, rest.get(1..)?)),
            _ => return None,
        }
    }
}

/// The TYPE one column declares, as written, and what follows it: every token up
/// to the comma or the closing parenthesis of the column list, so a parametrised
/// type (`VARCHAR(16)`, `DECIMAL(10,2)`) and a two-word one (`DOUBLE
/// PRECISION`) are read whole.
fn type_text(tokens: &[Token]) -> Option<(String, &[Token])> {
    let mut depth = 0_usize;
    let mut text = String::new();
    let mut previous_word = false;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            Token::LParen | Token::Lt => depth += 1,
            Token::RParen | Token::Gt if depth > 0 => depth -= 1,
            Token::RParen | Token::Comma if depth == 0 => {
                if text.is_empty() {
                    return None;
                }
                return Some((text, tokens.get(index..)?));
            }
            _ => {}
        }
        let word = matches!(token, Token::Word(_));
        if word && previous_word {
            text.push(' ');
        }
        text.push_str(&token.to_string());
        previous_word = word;
    }
    None
}

/// A parenthesised list of `'<key>' = '<value>'` properties, and what follows
/// it.
fn property_list(tokens: &[Token]) -> Option<(NamedValues, &[Token])> {
    if !matches!(tokens.first(), Some(Token::LParen)) {
        return None;
    }
    let mut properties: Vec<(String, String)> = Vec::new();
    let mut rest = tokens.get(1..)?;
    loop {
        let key = literal(rest.first()?).or_else(|| name(rest.first()?))?;
        if !matches!(rest.get(1)?, Token::Eq) {
            return None;
        }
        let value = literal(rest.get(2)?).or_else(|| name(rest.get(2)?))?;
        properties.push((key, value));
        rest = rest.get(3..)?;
        match rest.first()? {
            Token::Comma => rest = rest.get(1..)?,
            Token::RParen => return Some((properties, rest.get(1..)?)),
            _ => return None,
        }
    }
}

/// `SET [ SESSION | LOCAL ] <parameter> { TO | = } { <value> | DEFAULT }`, after
/// the leading `SET`.
fn set(tokens: &[Token]) -> Option<WarehouseStatement> {
    let mut rest = tokens;
    // "SET [ SESSION | LOCAL ]": both scopes set the parameter for the session
    // this statement runs on — a `LOCAL` one until the end of the transaction,
    // which this emulator's one-statement transactions end with the statement.
    if rest
        .first()
        .is_some_and(|token| is_keyword(token, "session") || is_keyword(token, "local"))
    {
        rest = rest.get(1..)?;
    }
    let name = name(rest.first()?)?;
    rest = rest.get(1..)?;
    match rest.first()? {
        Token::Eq => {}
        token if is_keyword(token, "to") => {}
        _ => return None,
    }
    rest = rest.get(1..)?;
    let (value, rest) = parameter_value(rest)?;
    if !rest.is_empty() {
        return None;
    }
    Some(WarehouseStatement::Set { name, value })
}

/// `RESET { <parameter> | ALL }`, after the leading `RESET`.
fn reset(tokens: &[Token]) -> Option<WarehouseStatement> {
    let [token] = tokens else {
        return None;
    };
    if is_keyword(token, "all") {
        return Some(WarehouseStatement::Reset { name: None });
    }
    Some(WarehouseStatement::Reset {
        name: Some(name(token)?),
    })
}

/// `SHOW { <parameter> | ALL }`, after the leading `SHOW`.
fn show(tokens: &[Token]) -> Option<WarehouseStatement> {
    let [token] = tokens else {
        return None;
    };
    if is_keyword(token, "all") {
        return Some(WarehouseStatement::Show { name: None });
    }
    Some(WarehouseStatement::Show {
        name: Some(name(token)?),
    })
}

/// The value a `SET` asks for, and what follows it: a string, a number, an
/// identifier, a comma-separated list of those (`SET datestyle TO ISO, MDY`), or
/// `None` for `DEFAULT`.
fn parameter_value(tokens: &[Token]) -> Option<(Option<String>, &[Token])> {
    if is_keyword(tokens.first()?, "default") {
        return Some((None, tokens.get(1..)?));
    }
    let mut values: Vec<String> = Vec::new();
    let mut rest = tokens;
    loop {
        values.push(scalar(rest.first()?)?);
        rest = rest.get(1..)?;
        match rest.first() {
            Some(Token::Comma) => rest = rest.get(1..)?,
            _ => return Some((Some(values.join(", ")), rest)),
        }
    }
}

/// Whether `tokens` starts with these keywords, in this order.
fn starts_with_keywords(tokens: &[Token], words: &[&str]) -> bool {
    tokens.len() >= words.len()
        && words
            .iter()
            .zip(tokens)
            .all(|(word, token)| is_keyword(token, word))
}

/// A clause that NAMES a value — `<keyword> [AS] <value>` — reported as what
/// follows the keyword and the optional noise word. `None` when the clause does
/// not start here.
fn value_clause<'a>(tokens: &'a [Token], keyword: &str) -> Option<&'a [Token]> {
    if !is_keyword(tokens.first()?, keyword) {
        return None;
    }
    let rest = tokens.get(1..)?;
    // Most of these clauses admit `AS` as a noise word ("DELIMITER [AS]
    // 'delimiter_char'").
    match rest.first() {
        Some(token) if is_keyword(token, "as") => rest.get(1..),
        _ => Some(rest),
    }
}

/// A clause that is a SWITCH — `<keyword> [ [=] { ON | OFF | TRUE | FALSE |
/// PRESET } ]` — reported as what follows it.
fn switch_clause<'a>(tokens: &'a [Token], keyword: &str) -> Option<&'a [Token]> {
    if !is_keyword(tokens.first()?, keyword) {
        return None;
    }
    let mut rest = tokens.get(1..)?;
    if matches!(rest.first(), Some(Token::Eq)) {
        rest = rest.get(1..)?;
    }
    const SETTINGS: [&str; 5] = ["on", "off", "true", "false", "preset"];
    if rest
        .first()
        .is_some_and(|token| SETTINGS.iter().any(|setting| is_keyword(token, setting)))
    {
        return rest.get(1..);
    }
    Some(rest)
}

/// `FORMAT [AS] { CSV | JSON … }`, after the keyword and its noise word: the
/// format and what follows it. A columnar format is not read ([`copy_option`]
/// says why).
fn data_format(tokens: &[Token]) -> Option<(DataFormat, &[Token])> {
    let head = tokens.first()?;
    if is_keyword(head, "csv") {
        return Some((DataFormat::Csv, tokens.get(1..)?));
    }
    if is_keyword(head, "json") {
        let rest = tokens.get(1..)?;
        // The same `'auto'` mapping [`copy_option`] reads for the bare keyword.
        if let Some(path) = rest.first().and_then(literal) {
            if !path.eq_ignore_ascii_case("auto") && !path.eq_ignore_ascii_case("auto ignorecase") {
                return None;
            }
            return Some((DataFormat::Json, rest.get(1..)?));
        }
        return Some((DataFormat::Json, rest));
    }
    None
}

/// The parenthesised column list a `COPY` may carry, and what follows it.
fn column_list(tokens: &[Token]) -> (Vec<String>, &[Token]) {
    if !matches!(tokens.first(), Some(Token::LParen)) {
        return (Vec::new(), tokens);
    }
    let mut columns: Vec<String> = Vec::new();
    let mut rest = &tokens[1..];
    loop {
        match rest.first() {
            Some(Token::RParen) => return (columns, &rest[1..]),
            Some(Token::Comma) => rest = &rest[1..],
            Some(token) => match name(token) {
                Some(column) => {
                    columns.push(column);
                    rest = &rest[1..];
                }
                // Not a column list after all: the caller reads the tokens as
                // they stand, which is a statement this module does not know.
                None => return (Vec::new(), tokens),
            },
            None => return (Vec::new(), tokens),
        }
    }
}

/// The string literal `token` carries. `None` for a token that is not one.
fn literal(token: &Token) -> Option<String> {
    match token {
        Token::SingleQuotedString(value) | Token::DoubleQuotedString(value) => {
            Some(value.clone())
        }
        _ => None,
    }
}

/// The name `token` carries — an identifier, quoted or not.
fn name(token: &Token) -> Option<String> {
    match token {
        Token::Word(word) => Some(word.value.clone()),
        _ => None,
    }
}

/// A scalar a `SET` may name: a string literal, a number, or an identifier.
fn scalar(token: &Token) -> Option<String> {
    match token {
        Token::SingleQuotedString(value) | Token::DoubleQuotedString(value) => Some(value.clone()),
        Token::Number(value, _) => Some(value.clone()),
        Token::Word(word) => Some(word.value.clone()),
        _ => None,
    }
}

/// The non-negative count `token` carries, as a number or as a string holding
/// one.
fn count(token: &Token) -> Option<u64> {
    match token {
        Token::Number(value, _) => value.parse().ok(),
        Token::SingleQuotedString(value) => value.parse().ok(),
        _ => None,
    }
}

/// The ONE character a delimiter or quote option names. A multi-character value
/// is not a delimiter ("the single ASCII character that is used to separate
/// fields"), so it is not read.
fn one_character(value: &str) -> Option<char> {
    let mut characters = value.chars();
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}
