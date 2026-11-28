// SPDX-License-Identifier: CC0-1.0

//! The CATALOG a SQL client reads a database's own shape out of: the
//! `information_schema` views every SQL engine publishes, served out of the
//! catalog of the engine behind one data-plane endpoint
//! ([`super::sql_engine`]).
//!
//! A client that has connected asks the server what the database holds — which
//! tables, which columns, of which type, and whether a column admits a null —
//! and it asks in SQL, over `information_schema`. So the views are REAL
//! relations of the engine, built from the engine's own catalog: every row is a
//! relation this database really holds and a column that relation really
//! declares, and nothing is answered from the query that asked.
//!
//! They are materialised on the engine's TEMPORARY schema immediately before a
//! statement that references one runs ([`materialise`]), for two reasons: the
//! catalog has moved on since the last statement read it, and a temporary
//! relation is not part of the database's own catalog — so `information_schema`
//! does not report itself.
//!
//! The columns each view carries are the ones the engine's catalog can answer
//! for truthfully, each named and typed as the SQL standard names and types it;
//! a column of the standard view that is a fact about storage this engine does
//! not keep is not carried at all rather than filled in with a guess. Which
//! views a product publishes BESIDE these is that product's own
//! ([`super::SqlDialect`]), and a dialect that serves one builds it from the
//! same reading of the catalog ([`DECLARED_COLUMNS_QUERY`]).

use super::sql_engine::{Cell, DatabaseEngine, EngineError, EngineView};

/// The schema the standard catalog views live in, which is the name a client
/// qualifies them with.
pub const INFORMATION_SCHEMA: &str = "information_schema";

/// The schema a relation this engine holds under an unqualified name is
/// reported in: the schema a PostgreSQL-derived engine resolves an unqualified
/// name in.
pub const DEFAULT_SCHEMA: &str = "public";

/// The catalog views this module serves, by the name a statement references
/// them under.
pub const CATALOG_VIEWS: &[&str] = &["information_schema.tables", "information_schema.columns"];

/// The relations of this database and the columns each one declares, as the
/// ENGINE's own catalog answers them.
///
/// One reading, so every catalog view — the standard ones here and the ones a
/// product publishes of its own — reports the same columns of the same
/// relations. Each row carries:
///
/// * `table_schema` / `table_name`: the relation. A relation of a schema is
///   held under its qualified name, so the name is split back into the two.
/// * `table_type`: `BASE TABLE` or `VIEW`, as the standard view classifies it.
/// * `column_name` and `ordinal_position`: the column and the position the
///   relation declares it at, counted from one.
/// * `column_default`: the default its declaration carries, and null for a
///   column that declares none.
/// * `is_nullable` / `not_null`: whether the declaration admits a null, in the
///   standard view's `YES`/`NO` spelling and as a flag.
/// * `data_type`: the type the column was declared with.
///
/// The relations it reads are the DATABASE's: the engine's own objects
/// (`sqlite_`) and the temporary relations of a connection (`pg_temp_`) are not
/// relations of the database's catalog, so neither is in any view built from
/// this.
pub const DECLARED_COLUMNS_QUERY: &str = concat!(
    "SELECT CASE WHEN instr(relation.name, '.') > 0 \
     THEN substr(relation.name, 1, instr(relation.name, '.') - 1) ELSE 'public' END \
     AS table_schema, \
     CASE WHEN instr(relation.name, '.') > 0 \
     THEN substr(relation.name, instr(relation.name, '.') + 1) ELSE relation.name END \
     AS table_name, \
     CASE relation.type WHEN 'view' THEN 'VIEW' ELSE 'BASE TABLE' END AS table_type, \
     declared.name AS column_name, declared.cid + 1 AS ordinal_position, \
     declared.dflt_value AS column_default, \
     CASE declared.\"notnull\" WHEN 0 THEN 'YES' ELSE 'NO' END AS is_nullable, \
     declared.\"notnull\" AS not_null, \
     lower(declared.type) AS data_type \
     FROM ",
    "sqlite_master",
    " AS relation JOIN pragma_table_info(relation.name) AS declared \
     WHERE relation.type IN ('table', 'view') \
     AND relation.name NOT LIKE 'sqlite_%' AND relation.name NOT LIKE 'pg\\_temp\\_%' ESCAPE '\\'",
);

/// Which of [`CATALOG_VIEWS`] the relations `relations` name, which are the ones
/// a statement needs materialised before it runs.
#[must_use]
pub fn referenced(relations: &[String]) -> Vec<&'static str> {
    CATALOG_VIEWS
        .iter()
        .copied()
        .filter(|view| {
            relations
                .iter()
                .any(|relation| relation.eq_ignore_ascii_case(view))
        })
        .collect()
}

/// One column of one relation, as the ENGINE's own catalog declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredColumn {
    /// The schema the relation is in.
    pub table_schema: String,
    /// The relation's own name.
    pub table_name: String,
    /// `BASE TABLE` or `VIEW`, as the standard view classifies it.
    pub table_type: String,
    /// The column's name.
    pub column_name: String,
    /// Its position in the relation, counted from one.
    pub ordinal_position: i64,
    /// The default its declaration carries, where it carries one.
    pub column_default: Option<String>,
    /// Whether the declaration refuses a null.
    pub not_null: bool,
    /// The type the column was declared with, as the declaration wrote it.
    pub declared_type: String,
}

impl DeclaredColumn {
    /// The column's type under the name the SQL standard gives it, without the
    /// length or precision modifier — which is what `data_type` carries, the
    /// modifier being reported in the columns beside it.
    #[must_use]
    pub fn data_type(&self) -> String {
        canonical_type_name(&self.declared_type)
    }

    /// The column's type under the standard name WITH the modifier its
    /// declaration carries (`character varying(64)`, `numeric(9,2)`), which is
    /// how a catalog view that reports one column per row spells a type.
    #[must_use]
    pub fn full_data_type(&self) -> String {
        let name = self.data_type();
        match type_modifier(&self.declared_type) {
            Some(modifier) => format!("{name}({modifier})"),
            None => name,
        }
    }

    /// `character_maximum_length`: the declared length of a character type, and
    /// `None` for every other type.
    #[must_use]
    pub fn character_maximum_length(&self) -> Option<i64> {
        if !is_character_type(&self.data_type()) {
            return None;
        }
        type_modifier(&self.declared_type)?.trim().parse().ok()
    }

    /// `numeric_precision` and `numeric_scale`: the two parts of an exact
    /// numeric type's modifier, and `None` for every other type.
    #[must_use]
    pub fn numeric_precision_and_scale(&self) -> (Option<i64>, Option<i64>) {
        if self.data_type() != "numeric" {
            return (None, None);
        }
        let Some(modifier) = type_modifier(&self.declared_type) else {
            return (None, None);
        };
        let (precision, scale) = match modifier.split_once(',') {
            Some((precision, scale)) => (precision, Some(scale)),
            None => (modifier, None),
        };
        (
            precision.trim().parse().ok(),
            scale.and_then(|scale| scale.trim().parse().ok()),
        )
    }
}

/// The types whose modifier is a LENGTH in characters.
///
/// Source: the SQL standard's `character_maximum_length` — "if `data_type`
/// identifies a character or bit string type, the declared maximum length".
fn is_character_type(canonical: &str) -> bool {
    matches!(canonical, "character" | "character varying")
}

/// The modifier a declared type carries, the text between its parentheses, or
/// `None` for a type declared without one.
fn type_modifier(declared: &str) -> Option<&str> {
    let (_, rest) = declared.split_once('(')?;
    rest.rsplit_once(')').map(|(modifier, _)| modifier)
}

/// The name the SQL standard gives the type a declaration names, which is the
/// name a catalog view reports and not the spelling the declaration used.
///
/// Source: the Amazon Redshift *Data types* reference, whose table lists the
/// aliases each type may be declared under beside the type's own name (`INT`,
/// `INT4` for `INTEGER`; `DECIMAL` for `NUMERIC`; `VARCHAR`, `NVARCHAR`, `TEXT`
/// for `CHARACTER VARYING`), and PostgreSQL's `information_schema`, which
/// reports the standard name. A type this list does not carry is reported as it
/// was declared, lower-cased: a name nothing here recognises is the client's own
/// and is not replaced by a guess.
#[must_use]
pub fn canonical_type_name(declared: &str) -> String {
    let lowered = declared.trim().to_ascii_lowercase();
    let base = lowered
        .split_once('(')
        .map_or(lowered.as_str(), |(base, _)| base)
        .trim();
    match base {
        "smallint" | "int2" => "smallint",
        "integer" | "int" | "int4" => "integer",
        "bigint" | "int8" => "bigint",
        "decimal" | "numeric" => "numeric",
        "real" | "float4" => "real",
        "double precision" | "float" | "float8" => "double precision",
        "boolean" | "bool" => "boolean",
        "char" | "character" | "nchar" | "bpchar" => "character",
        "varchar" | "character varying" | "nvarchar" | "text" => "character varying",
        "date" => "date",
        "time" | "time without time zone" => "time without time zone",
        "timetz" | "time with time zone" => "time with time zone",
        "timestamp" | "timestamp without time zone" => "timestamp without time zone",
        "timestamptz" | "timestamp with time zone" => "timestamp with time zone",
        other => other,
    }
    .to_string()
}

/// The columns of every relation this database holds, read through
/// [`DECLARED_COLUMNS_QUERY`].
pub fn declared_columns(
    engine: &DatabaseEngine,
    session: &mut EngineView,
) -> Result<Vec<DeclaredColumn>, EngineError> {
    let answered = engine.execute_as(session, DECLARED_COLUMNS_QUERY)?;
    Ok(answered
        .rows
        .iter()
        .filter_map(|row| {
            Some(DeclaredColumn {
                table_schema: text(row.first())?,
                table_name: text(row.get(1))?,
                table_type: text(row.get(2))?,
                column_name: text(row.get(3))?,
                ordinal_position: number(row.get(4))?,
                column_default: text(row.get(5)),
                not_null: number(row.get(7)).unwrap_or(0) != 0,
                declared_type: text(row.get(8)).unwrap_or_default(),
            })
        })
        .collect())
}

/// One text value of an answered row, and `None` for a null or a value of
/// another storage class.
fn text(cell: Option<&Cell>) -> Option<String> {
    match cell {
        Some(Cell::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

/// One integer value of an answered row.
fn number(cell: Option<&Cell>) -> Option<i64> {
    match cell {
        Some(Cell::Integer(number)) => Some(*number),
        _ => None,
    }
}

/// Put `views` on the engine's temporary schema, holding the catalog as it
/// stands now, with `catalog` reported as the database each relation is in.
///
/// Dropped and rebuilt per statement, because the catalog has moved on since the
/// last statement read it.
pub fn materialise(
    engine: &DatabaseEngine,
    session: &mut EngineView,
    catalog: &str,
    views: &[&'static str],
) -> Result<(), EngineError> {
    if views.is_empty() {
        return Ok(());
    }
    let columns = declared_columns(engine, session)?;
    for view in views {
        match *view {
            "information_schema.tables" => {
                materialise_tables(engine, session, catalog, &columns)?;
            }
            "information_schema.columns" => {
                materialise_columns(engine, session, catalog, &columns)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// `information_schema.tables` — one row for each table or view in the current
/// database.
fn materialise_tables(
    engine: &DatabaseEngine,
    session: &mut EngineView,
    catalog: &str,
    columns: &[DeclaredColumn],
) -> Result<(), EngineError> {
    let mut rows: Vec<String> = Vec::new();
    let mut seen: Vec<(String, String)> = Vec::new();
    for column in columns {
        let relation = (column.table_schema.clone(), column.table_name.clone());
        if seen.contains(&relation) {
            continue;
        }
        seen.push(relation);
        rows.push(format!(
            "({}, {}, {}, {})",
            literal(catalog),
            literal(&column.table_schema),
            literal(&column.table_name),
            literal(&column.table_type),
        ));
    }
    rebuild(
        engine,
        session,
        "information_schema.tables",
        "table_catalog VARCHAR, table_schema VARCHAR, table_name VARCHAR, table_type VARCHAR",
        &rows,
    )
}

/// `information_schema.columns` — one row for each column in a table or view of
/// the current database.
///
/// The columns carried are the ones the engine's catalog answers for: the
/// relation, the column and its position, its default, whether it admits a null,
/// its type under the standard's own name, and the length or the precision and
/// scale that type's declaration carried.
fn materialise_columns(
    engine: &DatabaseEngine,
    session: &mut EngineView,
    catalog: &str,
    columns: &[DeclaredColumn],
) -> Result<(), EngineError> {
    let rows: Vec<String> = columns
        .iter()
        .map(|column| {
        let (precision, scale) = column.numeric_precision_and_scale();
            format!(
                "({}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {})",
                literal(catalog),
                literal(&column.table_schema),
                literal(&column.table_name),
                literal(&column.column_name),
                column.ordinal_position,
                column
                    .column_default
                    .as_deref()
                    .map_or_else(|| "NULL".to_string(), literal),
                literal(if column.not_null { "NO" } else { "YES" }),
                literal(&column.data_type()),
                optional(column.character_maximum_length()),
                optional(precision),
                optional(scale),
            )
        })
        .collect();
    rebuild(
        engine,
        session,
        "information_schema.columns",
        "table_catalog VARCHAR, table_schema VARCHAR, table_name VARCHAR, \
         column_name VARCHAR, ordinal_position INTEGER, column_default VARCHAR, \
         is_nullable VARCHAR, data_type VARCHAR, character_maximum_length INTEGER, \
         numeric_precision INTEGER, numeric_scale INTEGER",
        &rows,
    )
}

/// Drop the temporary relation `name` and build it again, declared as `columns`
/// and holding `rows`.
///
/// The declaration is what a client reads each column's TYPE from, so every
/// view's columns are declared with the type the standard names for them rather
/// than left to the storage class of whatever the catalog answered.
pub fn rebuild(
    engine: &DatabaseEngine,
    session: &mut EngineView,
    name: &str,
    columns: &str,
    rows: &[String],
) -> Result<(), EngineError> {
    let held = quoted(name);
    engine.execute_as(session, &format!("DROP TABLE IF EXISTS temp.{held}"))?;
    engine.execute_as(session, &format!("CREATE TEMP TABLE {held} ({columns})"))?;
    // In batches, so one statement's text stays well inside the length the
    // engine accepts however many columns the database holds.
    for batch in rows.chunks(ROWS_PER_INSERT) {
        engine.execute_as(
            session,
            &format!("INSERT INTO temp.{held} VALUES {}", batch.join(", ")),
        )?;
    }
    Ok(())
}

/// How many rows one `INSERT` of a materialised view carries.
const ROWS_PER_INSERT: usize = 64;

/// A number as a SQL value, and `NULL` for one the catalog does not report.
fn optional(number: Option<i64>) -> String {
    number.map_or_else(|| "NULL".to_string(), |number| number.to_string())
}

/// `name` as one quoted identifier, which is what a name carrying the schema it
/// is in has to be: unquoted it would address a schema the engine does not
/// have.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `text` as a SQL string literal.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_wire_planes::sql_engine::Cell;

    /// A session view for a statement of the database `database`.
    fn view_of(database: &str) -> EngineView {
        EngineView {
            server_version: "8.0.2".to_string(),
            server_banner: String::new(),
            database: database.to_string(),
            user: "dbadmin".to_string(),
            connection_id: 1,
            last_insert_id: 0,
            settings: Vec::new(),
        }
    }

    /// The catalog views report the relations the database really holds, with
    /// the columns and nullability each one declares — and nothing of the
    /// engine's own.
    #[test]
    fn the_catalog_reports_what_the_database_holds() {
        let engine = DatabaseEngine::open().expect("the engine starts");
        let mut view = view_of("devdb");
        for ddl in [
            "CREATE TABLE orders (id INTEGER NOT NULL, label VARCHAR(16), total DECIMAL(9,2) DEFAULT 0)",
            "CREATE TABLE \"sales.regions\" (region VARCHAR(8) NOT NULL)",
            "CREATE VIEW labels AS SELECT label FROM orders",
        ] {
            engine
                .execute_as(&mut view, ddl)
                .unwrap_or_else(|error| panic!("setup {ddl:?}: {error:?}"));
        }
        materialise(&engine, &mut view, "devdb", CATALOG_VIEWS).expect("the catalog is built");

        let tables = engine
            .execute_as(
                &mut view,
                "SELECT table_schema, table_name, table_type FROM \
                 \"information_schema.tables\" ORDER BY table_schema, table_name",
            )
            .expect("the tables view answers");
        let rendered: Vec<Vec<Cell>> = tables.rows.clone();
        assert_eq!(
            rendered,
            vec![
                vec![
                    Cell::Text("public".to_string()),
                    Cell::Text("labels".to_string()),
                    Cell::Text("VIEW".to_string())
                ],
                vec![
                    Cell::Text("public".to_string()),
                    Cell::Text("orders".to_string()),
                    Cell::Text("BASE TABLE".to_string())
                ],
                vec![
                    Cell::Text("sales".to_string()),
                    Cell::Text("regions".to_string()),
                    Cell::Text("BASE TABLE".to_string())
                ],
            ],
            "case the tables view: expected the three relations the database holds, got {tables:?}"
        );

        let columns = engine
            .execute_as(
                &mut view,
                "SELECT column_name, ordinal_position, is_nullable, data_type, column_default, \
                 character_maximum_length, numeric_precision, numeric_scale \
                 FROM \"information_schema.columns\" WHERE table_name = 'orders' \
                 ORDER BY ordinal_position",
            )
            .expect("the columns view answers");
        assert_eq!(
            columns.rows,
            vec![
                vec![
                    Cell::Text("id".to_string()),
                    Cell::Integer(1),
                    Cell::Text("NO".to_string()),
                    Cell::Text("integer".to_string()),
                    Cell::Null,
                    Cell::Null,
                    Cell::Null,
                    Cell::Null,
                ],
                vec![
                    Cell::Text("label".to_string()),
                    Cell::Integer(2),
                    Cell::Text("YES".to_string()),
                    Cell::Text("character varying".to_string()),
                    Cell::Null,
                    Cell::Integer(16),
                    Cell::Null,
                    Cell::Null,
                ],
                vec![
                    Cell::Text("total".to_string()),
                    Cell::Integer(3),
                    Cell::Text("YES".to_string()),
                    Cell::Text("numeric".to_string()),
                    Cell::Text("0".to_string()),
                    Cell::Null,
                    Cell::Integer(9),
                    Cell::Integer(2),
                ],
            ],
            "case the columns view: expected the columns the table declares, got {columns:?}"
        );
        assert_eq!(
            columns
                .columns
                .first()
                .and_then(|column| column.declared_type.clone()),
            Some("VARCHAR".to_string()),
            "case the columns view: each column carries the type the standard names for it, got \
             {columns:?}"
        );
    }

    /// A declared type is reported under the name the standard gives it, with
    /// the length or the precision and scale its declaration carried read off
    /// it; a type nothing recognises is reported as it was declared.
    #[test]
    fn a_declared_type_is_reported_under_its_standard_name() {
        for (declared, name, full, length, precision, scale) in [
            ("INT", "integer", "integer", None, None, None),
            ("int4", "integer", "integer", None, None, None),
            ("BIGINT", "bigint", "bigint", None, None, None),
            ("int2", "smallint", "smallint", None, None, None),
            (
                "VARCHAR(64)",
                "character varying",
                "character varying(64)",
                Some(64),
                None,
                None,
            ),
            (
                "text",
                "character varying",
                "character varying",
                None,
                None,
                None,
            ),
            ("CHAR(2)", "character", "character(2)", Some(2), None, None),
            (
                "DECIMAL(9,2)",
                "numeric",
                "numeric(9,2)",
                None,
                Some(9),
                Some(2),
            ),
            ("numeric(18)", "numeric", "numeric(18)", None, Some(18), None),
            ("bool", "boolean", "boolean", None, None, None),
            (
                "float8",
                "double precision",
                "double precision",
                None,
                None,
                None,
            ),
            (
                "TIMESTAMP",
                "timestamp without time zone",
                "timestamp without time zone",
                None,
                None,
                None,
            ),
            // A type of the product's own, which this list does not carry: it
            // is reported as the declaration wrote it.
            ("SUPER", "super", "super", None, None, None),
        ] {
            let column = DeclaredColumn {
                table_schema: DEFAULT_SCHEMA.to_string(),
                table_name: "t".to_string(),
                table_type: "BASE TABLE".to_string(),
                column_name: "c".to_string(),
                ordinal_position: 1,
                column_default: None,
                not_null: false,
                declared_type: declared.to_string(),
            };
            assert_eq!(
                (
                    column.data_type(),
                    column.full_data_type(),
                    column.character_maximum_length(),
                    column.numeric_precision_and_scale(),
                ),
                (
                    name.to_string(),
                    full.to_string(),
                    length,
                    (precision, scale)
                ),
                "case {declared:?}: expected {name:?} / {full:?} / {length:?} / \
                 {precision:?},{scale:?}"
            );
        }
    }

    /// Only the views a statement references are built, and a statement naming
    /// none builds nothing.
    #[test]
    fn only_a_referenced_view_is_named() {
        for (case, relations, expected) in [
            ("none", vec!["orders".to_string()], Vec::new()),
            (
                "the columns view",
                vec!["information_schema.columns".to_string()],
                vec!["information_schema.columns"],
            ),
            (
                "either case",
                vec!["INFORMATION_SCHEMA.TABLES".to_string()],
                vec!["information_schema.tables"],
            ),
            (
                "both",
                vec![
                    "information_schema.tables".to_string(),
                    "information_schema.columns".to_string(),
                ],
                vec!["information_schema.tables", "information_schema.columns"],
            ),
        ] {
            assert_eq!(
                referenced(&relations),
                expected,
                "case {case:?}: expected {expected:?} for {relations:?}"
            );
        }
    }
}
