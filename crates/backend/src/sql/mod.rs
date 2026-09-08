//! The mirage: user SQL becomes a query ClickHouse may run. An org's "tables" are rewrites of
//! the one physical `space_station.records`; nothing else is reachable through a query.
//!
//! `plan` parses and validates (one SELECT; an allowlist of FROM items; CTE names scoped the
//! way ClickHouse resolves them; no `SETTINGS`, `FORMAT` or `INTO`; identifiers in `IN (…)`
//! must be columns), `Plan::render` rewrites every table reference to the bounded mirage
//! subquery, and `trigger_plan` / `trigger_sql` build the `SELECT 1 … LIMIT 1` probe run after
//! each flush. The rewrite is the mirage; the `ss_query` profile and row policy are the
//! boundary. See docs/ARCHITECTURE.md, "The mirage".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::ControlFlow::{self, Break, Continue};

use space_station_shared::secrets::valid_table_id;
use sqlparser::ast::{
    ArrayElemTypeDef, DataType, Expr, FunctionArg, FunctionArgExpr, FunctionArguments, Ident, JoinOperator, ObjectName,
    ObjectNamePart, Query, Select, SetExpr, Statement, TableAlias, TableFactor, VisitMut, VisitorMut, With,
};
use sqlparser::dialect::ClickHouseDialect;
use sqlparser::parser::{Parser, ParserError};
use sqlparser::tokenizer::Token;

#[cfg(test)]
mod tests;

/// SQL (or a trigger `where`) longer than this is refused unparsed.
pub const SQL_MAX: usize = 64 * 1024;

const DIALECT: ClickHouseDialect = ClickHouseDialect {};

/// ClickHouse reads an identifier as the second argument of these as a table, exactly like
/// `x IN (name)`. Function names are case-sensitive there.
const IN_FUNCTIONS: [&str; 8] =
    ["in", "notIn", "globalIn", "globalNotIn", "nullIn", "notNullIn", "globalNullIn", "globalNotNullIn"];

/// Why a query was refused. `Display` is the one-line, user-facing message; `code` is stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardError {
    Parse(String),
    NotAQuery,
    MultipleStatements,
    Settings,
    Format,
    IntoOutfile,
    TableFunction(String),
    QualifiedName(String),
    UnknownTable(String),
    Forbidden(String),
    TooLong,
}

impl GuardError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Parse(_) => "parse",
            Self::NotAQuery => "not_a_query",
            Self::MultipleStatements => "multiple_statements",
            Self::Settings => "settings",
            Self::Format => "format",
            Self::IntoOutfile => "into_outfile",
            Self::TableFunction(_) => "table_function",
            Self::QualifiedName(_) => "qualified_name",
            Self::UnknownTable(_) => "unknown_table",
            Self::Forbidden(_) => "forbidden",
            Self::TooLong => "too_long",
        }
    }
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "could not parse the SQL: {e}"),
            Self::NotAQuery => f.write_str("only a single SELECT query is allowed"),
            Self::MultipleStatements => f.write_str("only one statement is allowed"),
            Self::Settings => f.write_str("SETTINGS is not allowed"),
            Self::Format => f.write_str("FORMAT is not allowed; rows always come back as JSON"),
            Self::IntoOutfile => f.write_str("INTO is not allowed"),
            Self::TableFunction(name) => write!(f, "table function {name}() is not allowed"),
            Self::QualifiedName(name) => write!(f, "{name}: only this org's tables can be read, by bare table id"),
            Self::UnknownTable(name) => write!(f, "unknown table {name}"),
            Self::Forbidden(what) => write!(f, "{what} is not allowed"),
            Self::TooLong => write!(f, "the SQL is longer than {} KB", SQL_MAX / 1024),
        }
    }
}

impl std::error::Error for GuardError {}

impl From<ParserError> for GuardError {
    fn from(e: ParserError) -> Self {
        Self::Parse(match e {
            ParserError::TokenizerError(s) | ParserError::ParserError(s) => s,
            ParserError::RecursionLimitExceeded => "the query is nested too deeply".into(),
        })
    }
}

/// Cursor bounds for one table: `(from, to]`, either side open when `None`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bounds {
    pub from: Option<u64>,
    pub to: Option<u64>,
}

/// A parsed, validated query, ready to render for any org and bounds.
#[derive(Debug, Clone)]
pub struct Plan {
    query: Query,
    tables: BTreeSet<String>,
}

impl Plan {
    fn new(mut query: Query, visible: &dyn Fn(&str) -> bool) -> Result<Self, GuardError> {
        let tables = Guard::run(&mut query, visible, None)?;
        Ok(Self { query, tables })
    }

    /// Every org table referenced (CTE names excluded); the caller fetches their watermarks.
    pub fn tables(&self) -> &BTreeSet<String> {
        &self.tables
    }

    /// The SQL to send: every table reference is the mirage subquery bounded by `bounds[table]`
    /// (absent = unbounded), then ` FORMAT JSONEachRow`. An `org` that is not a valid id matches
    /// nothing.
    pub fn render(&self, org: &str, bounds: &BTreeMap<String, Bounds>) -> String {
        format!("{} FORMAT JSONEachRow", self.sql(org, bounds))
    }

    /// `render` without the FORMAT: the query alone, for a caller that wraps it (`DESCRIBE (…)`).
    pub fn sql(&self, org: &str, bounds: &BTreeMap<String, Bounds>) -> String {
        let mut query = self.query.clone();
        Guard::run(&mut query, &|t| self.tables.contains(t), Some(Mirage { org, bounds }))
            .expect("plan() accepted this query under the same rules");
        query.to_string()
    }
}

/// Parse and validate `sql`. `visible(table_id)` says whether the caller may see that org table.
pub fn plan(sql: &str, visible: &dyn Fn(&str) -> bool) -> Result<Plan, GuardError> {
    if sql.len() > SQL_MAX {
        return Err(GuardError::TooLong);
    }
    let mut statements = Parser::parse_sql(&DIALECT, sql)?;
    if statements.len() > 1 {
        return Err(GuardError::MultipleStatements);
    }
    match statements.pop() {
        Some(Statement::Query(query)) => Plan::new(*query, visible),
        _ => Err(GuardError::NotAQuery),
    }
}

/// The probe run after a flush, `SELECT 1 FROM <table> [WHERE (<where_>)] LIMIT 1`, as a plan:
/// a trigger parses once and renders per flush. `where_` is parsed with `Parser::parse_expr`
/// and spliced into the AST, never as text, and obeys every rule above (a subquery in it may
/// read any `visible` table, unbounded); `None` matches any row.
pub fn trigger_plan(table: &str, where_: Option<&str>, visible: &dyn Fn(&str) -> bool) -> Result<Plan, GuardError> {
    if !valid_table_id(table) {
        return Err(GuardError::UnknownTable(table.into()));
    }
    let mut query = Parser::new(&DIALECT).try_with_sql(&format!("SELECT 1 FROM {table} LIMIT 1"))?.parse_query()?;
    if let Some(where_) = where_ {
        if where_.len() > SQL_MAX {
            return Err(GuardError::TooLong);
        }
        let mut parser = Parser::new(&DIALECT).try_with_sql(where_)?;
        let expr = parser.parse_expr()?;
        parser.expect_token(&Token::EOF)?;
        if let SetExpr::Select(select) = &mut *query.body {
            select.selection = Some(Expr::Nested(Box::new(expr)));
        }
    }
    Plan::new(*query, visible)
}

/// `trigger_plan` rendered over one flush: one row when `(from, to]` of `table` holds a hit.
/// Every org table is visible here: a notification was checked with its saver's identity when
/// it was saved.
pub fn trigger_sql(org: &str, table: &str, where_: Option<&str>, from: u64, to: u64) -> Result<String, GuardError> {
    if !valid_org(org) {
        return Err(GuardError::Forbidden(format!("org id {org:?}")));
    }
    let bounds = BTreeMap::from([(table.into(), Bounds { from: Some(from), to: Some(to) })]);
    Ok(trigger_plan(table, where_, &|_| true)?.render(org, &bounds))
}

/// Org ids are `^[a-z0-9_-]{3,50}$`.
pub fn valid_org(id: &str) -> bool {
    (3..=50).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// What a rendering rewrites tables with; absent while planning.
struct Mirage<'a> {
    org: &'a str,
    bounds: &'a BTreeMap<String, Bounds>,
}

/// The one physical relation every mirage reads. An org may own a table with this id — nothing
/// reserves it — which is exactly why every table reference is rewritten and a CTE named after it
/// is refused rather than trusted.
pub const PHYSICAL: &str = "records";

impl Mirage<'_> {
    /// `(SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM
    /// space_station.records WHERE org_id = '…' AND table_id = '…' [AND cursor > from]
    /// [AND cursor <= to]) AS alias`, the alias being the user's or the table id.
    fn factor(&self, table: &str, alias: Option<TableAlias>) -> TableFactor {
        let org = if valid_org(self.org) { self.org } else { "" };
        let Bounds { from, to } = self.bounds.get(table).copied().unwrap_or_default();
        let from = from.map(|c| format!(" AND cursor > {c}")).unwrap_or_default();
        let to = to.map(|c| format!(" AND cursor <= {c}")).unwrap_or_default();
        let sql = format!(
            "SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record \
             FROM space_station.{PHYSICAL} WHERE org_id = '{org}' AND table_id = '{table}'{from}{to}"
        );
        let subquery = Parser::new(&DIALECT)
            .try_with_sql(&sql)
            .and_then(|mut p| p.parse_query())
            .expect("the mirage template parses");
        let alias = alias.unwrap_or_else(|| TableAlias {
            explicit: true,
            name: Ident::new(table),
            columns: Vec::new(),
            at: None,
        });
        TableFactor::Derived {
            lateral: false,
            subquery,
            alias: Some(TableAlias { explicit: true, ..alias }),
            sample: None,
        }
    }
}

/// One query's names: its CTEs, what its FROM items are called (aliases, else table names), and
/// its `WITH` clause while the CTE bodies are visited by hand. ClickHouse resolves every CTE
/// name of a `WITH` inside every body except the CTE's own, which is the physical table unless
/// `WITH RECURSIVE` — and even then when the CTE is named `PHYSICAL`: `hidden` is that name
/// while its body is visited.
#[derive(Default)]
struct Scope {
    ctes: Vec<String>,
    hidden: Option<String>,
    names: Vec<String>,
    with: Option<With>,
}

/// The visitor. Validates on every run; rewrites tables to the mirage when `mirage` is set.
struct Guard<'a> {
    visible: &'a dyn Fn(&str) -> bool,
    mirage: Option<Mirage<'a>>,
    scope: Vec<Scope>,
    tables: BTreeSet<String>,
}

impl<'a> Guard<'a> {
    /// Validate `query` (rewriting its tables when `mirage` is given); the org tables it reads.
    fn run(
        query: &mut Query,
        visible: &'a dyn Fn(&str) -> bool,
        mirage: Option<Mirage<'a>>,
    ) -> Result<BTreeSet<String>, GuardError> {
        let mut guard = Guard { visible, mirage, scope: Vec::new(), tables: BTreeSet::new() };
        match query.visit(&mut guard) {
            Continue(()) => Ok(guard.tables),
            Break(e) => Err(e),
        }
    }

    /// The scope of the query being visited.
    fn scope(&mut self) -> &mut Scope {
        self.scope.last_mut().expect("selects and CTEs are visited inside a query")
    }

    fn is_cte(&self, name: &str) -> bool {
        self.scope.iter().any(|s| s.hidden.as_deref() != Some(name) && s.ctes.iter().any(|c| c == name))
    }

    /// May `root` start a column reference? A FROM name, a CTE, or the JSON columns themselves.
    fn is_column_root(&self, root: &str) -> bool {
        matches!(root, "record" | "metadata")
            || self.is_cte(root)
            || self.scope.iter().any(|s| s.names.iter().any(|n| n == root))
    }

    /// The FROM allowlist. A bare single-part name is a CTE in scope (left alone) or a visible
    /// table (recorded; rewritten when rendering); the relation of an ARRAY JOIN is a column
    /// expression, not a table; a derived table was checked from the inside. Nothing else passes.
    fn table(&mut self, factor: &mut TableFactor, array_join: bool) -> Result<(), GuardError> {
        let TableFactor::Table {
            name,
            alias,
            args: None,
            with_hints,
            version: None,
            with_ordinality: false,
            partitions,
            json_path: None,
            sample: None,
            index_hints,
        } = factor
        else {
            return match factor {
                TableFactor::Derived { .. } => Ok(()),
                TableFactor::Table { name, args: Some(_), .. } | TableFactor::Function { name, .. } => {
                    Err(GuardError::TableFunction(name.to_string()))
                }
                TableFactor::Table { .. } => Err(GuardError::Forbidden("a table modifier".into())),
                _ => Err(GuardError::Forbidden("this FROM syntax".into())),
            };
        };
        if !(with_hints.is_empty() && partitions.is_empty() && index_hints.is_empty()) {
            return Err(GuardError::Forbidden("a table modifier".into()));
        }
        if array_join {
            return Ok(());
        }
        let table = match name.0.as_slice() {
            [ObjectNamePart::Identifier(ident)] => ident.value.clone(),
            _ => return Err(GuardError::QualifiedName(name.to_string())),
        };
        if self.is_cte(&table) {
            return Ok(());
        }
        if !valid_table_id(&table) || !(self.visible)(&table) {
            return Err(GuardError::UnknownTable(table));
        }
        if let Some(mirage) = &self.mirage {
            *factor = mirage.factor(&table, alias.take());
        }
        self.tables.insert(table);
        Ok(())
    }
}

impl VisitorMut for Guard<'_> {
    type Break = GuardError;

    /// The statement itself is entered as a `Query`; a statement met on the way is DML nested
    /// in a query body.
    fn pre_visit_statement(&mut self, _: &mut Statement) -> ControlFlow<GuardError> {
        Break(GuardError::NotAQuery)
    }

    /// Refuse `SETTINGS`, `FORMAT` and non-SELECT bodies at every nesting level, then visit the
    /// CTE bodies one by one with each one's own name hidden. `with` is taken out so the visitor
    /// does not walk the bodies a second time; `post_visit_query` puts it back.
    fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<GuardError> {
        if query.settings.is_some() {
            return Break(GuardError::Settings);
        }
        if query.format_clause.is_some() {
            return Break(GuardError::Format);
        }
        if !selects_only(&query.body) {
            return Break(GuardError::NotAQuery);
        }
        let mut with = query.with.take();
        let ctes = with.iter().flat_map(|w| &w.cte_tables).map(|c| c.alias.name.value.clone()).collect();
        self.scope.push(Scope { ctes, ..Scope::default() });
        if let Some(w) = &mut with {
            for cte in &mut w.cte_tables {
                // ClickHouse resolves a non-recursive CTE's own name inside its own body to the
                // physical relation, so that self-reference must go through the allowlist. A
                // recursive one resolves to the CTE — except when it is named after the physical
                // table, which ClickHouse still reads.
                let name = &cte.alias.name.value;
                self.scope().hidden = (!w.recursive || name == PHYSICAL).then(|| name.clone());
                cte.query.visit(self)?;
            }
            self.scope().hidden = None;
        }
        self.scope().with = with;
        Continue(())
    }

    fn post_visit_query(&mut self, query: &mut Query) -> ControlFlow<GuardError> {
        query.with = self.scope.pop().and_then(|s| s.with);
        Continue(())
    }

    /// Before the select's expressions: refuse `INTO` and `PREWHERE` (which ClickHouse cannot
    /// apply to a subquery, and every table here is one); bind its FROM names for `is_column_root`.
    fn pre_visit_select(&mut self, select: &mut Select) -> ControlFlow<GuardError> {
        if select.into.is_some() {
            return Break(GuardError::IntoOutfile);
        }
        if select.prewhere.is_some() {
            return Break(GuardError::Forbidden("PREWHERE".into()));
        }
        let factors =
            select.from.iter().flat_map(|t| std::iter::once(&t.relation).chain(t.joins.iter().map(|j| &j.relation)));
        let names = factors.filter_map(|factor| match factor {
            TableFactor::Table { alias: Some(a), .. } | TableFactor::Derived { alias: Some(a), .. } => {
                Some(a.name.value.clone())
            }
            TableFactor::Table { name, .. } => {
                name.0.first().and_then(ObjectNamePart::as_ident).map(|i| i.value.clone())
            }
            _ => None,
        });
        self.scope().names.extend(names);
        Continue(())
    }

    /// After the select's children (so a derived table is validated from the inside first):
    /// the FROM allowlist and the rewrite, which the visitor then does not descend into.
    fn post_visit_select(&mut self, select: &mut Select) -> ControlFlow<GuardError> {
        for t in &mut select.from {
            let joins = t.joins.iter_mut().map(|j| {
                let array_join = matches!(
                    j.join_operator,
                    JoinOperator::ArrayJoin | JoinOperator::LeftArrayJoin | JoinOperator::InnerArrayJoin
                );
                (&mut j.relation, array_join)
            });
            for (factor, array_join) in std::iter::once((&mut t.relation, false)).chain(joins) {
                if let Err(e) = self.table(factor, array_join) {
                    return Break(e);
                }
            }
        }
        Continue(())
    }

    /// Casts keep ClickHouse's type spelling; an identifier in `IN (…)` (or the `in()` family)
    /// must be a column, since ClickHouse would otherwise read it as a table.
    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<GuardError> {
        let sets: Vec<&Expr> = match expr {
            Expr::Cast { data_type, .. } => {
                clickhouse_type(data_type);
                return Continue(());
            }
            Expr::InList { list, .. } => list.iter().collect(),
            Expr::Function(f) if is_in_function(&f.name) => match &f.args {
                FunctionArguments::List(list) => list
                    .args
                    .iter()
                    .skip(1)
                    .filter_map(|a| match a {
                        FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => Some(e),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
            _ => return Continue(()),
        };
        match sets.into_iter().find(|e| ident_root(e).is_some_and(|root| !self.is_column_root(root))) {
            Some(e) => Break(GuardError::Forbidden(format!("IN ({e})"))),
            None => Continue(()),
        }
    }
}

/// `SELECT`s, set operations of them and parenthesised queries; never `VALUES` or `TABLE t`.
fn selects_only(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(_) | SetExpr::Query(_) => true,
        SetExpr::SetOperation { left, right, .. } => selects_only(left) && selects_only(right),
        _ => false,
    }
}

fn is_in_function(name: &ObjectName) -> bool {
    matches!(name.0.as_slice(), [ObjectNamePart::Identifier(n)] if IN_FUNCTIONS.contains(&n.value.as_str()))
}

/// The first identifier of a bare or compound identifier, through parentheses.
fn ident_root(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Identifier(id) => Some(&id.value),
        Expr::CompoundIdentifier(parts) => parts.first().map(|i| i.value.as_str()),
        Expr::Nested(inner) => ident_root(inner),
        _ => None,
    }
}

/// sqlparser prints some type names in another dialect's spelling (`INT8`, `INT64`, `FLOAT64`,
/// `STRING`, `ENUM8(…)`) and ClickHouse type names are case-sensitive: restore them, inside
/// `Nullable`, `LowCardinality`, `Array`, `Map` and `Tuple` too. Every other spelling ClickHouse
/// accepts as printed (`DATE`, `DATETIME`, `DECIMAL`, `BOOL`, … are its case-insensitive aliases).
fn clickhouse_type(data_type: &mut DataType) {
    let name = match data_type {
        DataType::Int8(None) => "Int8",
        DataType::Int64 => "Int64",
        DataType::Float64 => "Float64",
        DataType::String(None) => "String",
        // `ENUM(…)` is a ClickHouse alias that picks the width itself; `ENUM8(…)` is nothing.
        DataType::Enum(_, bits) => return *bits = None,
        DataType::Nullable(inner)
        | DataType::LowCardinality(inner)
        | DataType::Array(ArrayElemTypeDef::Parenthesis(inner)) => return clickhouse_type(inner),
        DataType::Map(key, value) => {
            clickhouse_type(key);
            return clickhouse_type(value);
        }
        DataType::Tuple(fields) => {
            return fields.iter_mut().for_each(|f| clickhouse_type(&mut f.field_type));
        }
        _ => return,
    };
    *data_type = DataType::Custom(ObjectName::from(vec![Ident::new(name)]), Vec::new());
}
