//! The guard's tests: what it accepts and how it rewrites, what it refuses and with which
//! variant, bounds, trigger probes, error surface, and the rendered SQL running on ClickHouse
//! as the `ss_query` user behind the row policy.

use super::*;

/// The mirage of `table` for org `acme` with no bounds, aliased `alias`.
fn mirage(table: &str, alias: &str) -> String {
    format!(
        "(SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM space_station.records \
         WHERE org_id = 'acme' AND table_id = '{table}') AS {alias}"
    )
}

fn visible(table: &str) -> bool {
    matches!(table, "orders" | "customers")
}

fn planned(sql: &str) -> Plan {
    plan(sql, &visible).unwrap_or_else(|e| panic!("{sql}: {e}"))
}

fn refused(sql: &str) -> GuardError {
    plan(sql, &visible).err().unwrap_or_else(|| panic!("{sql}: accepted"))
}

fn rendered(sql: &str) -> String {
    planned(sql).render("acme", &BTreeMap::new())
}

fn tables(sql: &str) -> Vec<String> {
    planned(sql).tables().iter().cloned().collect()
}

mod accepts {
    use super::*;

    #[test]
    fn bare_table_becomes_the_mirage_aliased_by_its_own_name() {
        assert_eq!(
            rendered("select * from orders"),
            format!("SELECT * FROM {} FORMAT JSONEachRow", mirage("orders", "orders"))
        );
        assert_eq!(tables("SELECT * FROM orders"), ["orders"]);
    }

    #[test]
    fn user_alias_is_kept_with_or_without_as() {
        let expected = format!("SELECT o.cursor FROM {} FORMAT JSONEachRow", mirage("orders", "o"));
        assert_eq!(rendered("SELECT o.cursor FROM orders o"), expected);
        assert_eq!(rendered("SELECT o.cursor FROM orders AS o"), expected);
    }

    #[test]
    fn quoted_names_behave_like_bare_ones_and_quoted_aliases_stay_escaped() {
        let bare = rendered("SELECT * FROM orders");
        assert_eq!(rendered("SELECT * FROM `orders`"), bare);
        assert_eq!(rendered("SELECT * FROM \"orders\""), bare);
        for alias in ["\"a\"\"b\"", "`a``b`"] {
            assert_eq!(
                rendered(&format!("SELECT * FROM orders AS {alias}")),
                format!("SELECT * FROM {} FORMAT JSONEachRow", mirage("orders", alias))
            );
        }
    }

    #[test]
    fn join_rewrites_both_tables_and_lists_both() {
        let sql = "SELECT * FROM orders o JOIN customers c ON c.record.id = o.record.customer";
        assert_eq!(
            rendered(sql),
            format!(
                "SELECT * FROM {} JOIN {} ON c.record.id = o.record.customer FORMAT JSONEachRow",
                mirage("orders", "o"),
                mirage("customers", "c")
            )
        );
        assert_eq!(tables(sql), ["customers", "orders"]);
    }

    #[test]
    fn cte_body_is_rewritten_and_the_cte_reference_is_left_alone() {
        let sql = "WITH t AS (SELECT * FROM orders) SELECT * FROM t";
        assert_eq!(
            rendered(sql),
            format!("WITH t AS (SELECT * FROM {}) SELECT * FROM t FORMAT JSONEachRow", mirage("orders", "orders"))
        );
        assert_eq!(tables(sql), ["orders"]);
    }

    #[test]
    fn nested_derived_tables_are_rewritten_inside() {
        assert_eq!(
            rendered("SELECT * FROM (SELECT * FROM (SELECT * FROM orders) AS a) AS b"),
            format!(
                "SELECT * FROM (SELECT * FROM (SELECT * FROM {}) AS a) AS b FORMAT JSONEachRow",
                mirage("orders", "orders")
            )
        );
    }

    #[test]
    fn union_all_of_two_visible_tables() {
        let sql = "SELECT cursor FROM orders UNION ALL SELECT cursor FROM customers";
        assert_eq!(
            rendered(sql),
            format!(
                "SELECT cursor FROM {} UNION ALL SELECT cursor FROM {} FORMAT JSONEachRow",
                mirage("orders", "orders"),
                mirage("customers", "customers")
            )
        );
        assert_eq!(tables(sql), ["customers", "orders"]);
    }

    #[test]
    fn array_join_relation_is_a_column_expression_not_a_table() {
        for kind in ["ARRAY JOIN", "LEFT ARRAY JOIN"] {
            assert_eq!(
                rendered(&format!("SELECT i FROM orders {kind} record.items AS i")),
                format!("SELECT i FROM {} {kind} record.items AS i FORMAT JSONEachRow", mirage("orders", "orders"))
            );
        }
    }

    #[test]
    fn subqueries_on_other_visible_tables_are_rewritten_too() {
        let sql = "SELECT (SELECT count() FROM customers) FROM orders WHERE record.customer IN (SELECT record.id FROM customers)";
        let out = rendered(sql);
        assert!(out.contains(&mirage("customers", "customers")) && out.contains(&mirage("orders", "orders")), "{out}");
        assert_eq!(tables(sql), ["customers", "orders"]);
    }

    #[test]
    fn a_cte_named_records_never_reaches_the_physical_table() {
        let sql = "WITH records AS (SELECT 1) SELECT * FROM records";
        assert_eq!(rendered(sql), "WITH records AS (SELECT 1) SELECT * FROM records FORMAT JSONEachRow");
        assert!(tables(sql).is_empty());
    }

    #[test]
    fn a_cte_shadows_a_visible_table_of_the_same_name_as_clickhouse_does() {
        let sql = "WITH orders AS (SELECT 1) SELECT * FROM orders";
        assert_eq!(rendered(sql), "WITH orders AS (SELECT 1) SELECT * FROM orders FORMAT JSONEachRow");
        assert!(tables(sql).is_empty());
    }

    #[test]
    fn a_cte_body_naming_itself_reads_the_org_table_as_clickhouse_does() {
        let sql = "WITH orders AS (SELECT * FROM orders) SELECT * FROM orders";
        assert_eq!(
            rendered(sql),
            format!(
                "WITH orders AS (SELECT * FROM {}) SELECT * FROM orders FORMAT JSONEachRow",
                mirage("orders", "orders")
            )
        );
        assert_eq!(tables(sql), ["orders"]);
        assert_eq!(
            rendered("WITH orders AS (SELECT * FROM (SELECT * FROM orders) AS x) SELECT * FROM orders"),
            format!(
                "WITH orders AS (SELECT * FROM (SELECT * FROM {}) AS x) SELECT * FROM orders FORMAT JSONEachRow",
                mirage("orders", "orders")
            )
        );
    }

    #[test]
    fn ctes_see_each_other_in_any_order_as_clickhouse_does() {
        let sql = "WITH a AS (SELECT * FROM b), b AS (SELECT * FROM orders) SELECT * FROM a";
        assert_eq!(
            rendered(sql),
            format!(
                "WITH a AS (SELECT * FROM b), b AS (SELECT * FROM {}) SELECT * FROM a FORMAT JSONEachRow",
                mirage("orders", "orders")
            )
        );
        assert_eq!(tables(sql), ["orders"]);
    }

    #[test]
    fn a_recursive_cte_names_itself() {
        let sql = "WITH RECURSIVE t AS (SELECT 1 AS n UNION ALL SELECT n + 1 FROM t WHERE n < 3) SELECT * FROM t";
        assert_eq!(rendered(sql), format!("{sql} FORMAT JSONEachRow"));
        assert!(tables(sql).is_empty());
    }

    #[test]
    fn a_cte_inside_a_derived_table_shadows_only_inside_it() {
        assert_eq!(
            rendered("SELECT * FROM (WITH orders AS (SELECT 1) SELECT * FROM orders) AS d JOIN orders ON 1 = 1"),
            format!(
                "SELECT * FROM (WITH orders AS (SELECT 1) SELECT * FROM orders) AS d JOIN {} ON 1 = 1 FORMAT JSONEachRow",
                mirage("orders", "orders")
            )
        );
        let sql = "WITH t AS (SELECT * FROM orders) SELECT * FROM (WITH t AS (SELECT * FROM t) SELECT * FROM t) AS d";
        assert_eq!(
            rendered(sql),
            format!(
                "WITH t AS (SELECT * FROM {}) SELECT * FROM (WITH t AS (SELECT * FROM t) SELECT * FROM t) AS d \
                 FORMAT JSONEachRow",
                mirage("orders", "orders")
            ),
            "an inner CTE's own name is the outer CTE, as in ClickHouse"
        );
        assert_eq!(tables(sql), ["orders"]);
    }

    #[test]
    fn identifiers_in_in_lists_may_be_columns_or_ctes() {
        planned("SELECT * FROM orders o WHERE 1 IN (o.cursor, record.x, metadata.y)");
        planned("SELECT * FROM orders WHERE 1 IN (orders.cursor, (record.x))");
        planned("WITH t AS (SELECT 1) SELECT * FROM orders WHERE cursor IN (t)");
        planned("SELECT * FROM orders WHERE 1 IN (1, 2) AND 1 IN (currentDatabase())");
        planned("SELECT * FROM orders WHERE 1 IN (SELECT cursor FROM customers)");
    }

    #[test]
    fn casts_keep_clickhouse_type_spelling() {
        let out = rendered(
            "SELECT record.price::Float64, record.a::String, x::Int64, x::Int8, x::Nullable(Array(Float64)), \
             x::Map(String, Int64), x::Tuple(Int64, String), x::LowCardinality(String), x::UInt64, x::Date, \
             CAST(x AS Float64), arrayJoin(record.items::Array(String)), x::Enum8('a' = 1), x::Array(Enum16('b' = 2)) \
             FROM orders",
        );
        assert!(
            out.starts_with(
                "SELECT record.price::Float64, record.a::String, x::Int64, x::Int8, x::Nullable(Array(Float64)), \
                 x::Map(String, Int64), x::Tuple(Int64, String), x::LowCardinality(String), x::UInt64, x::DATE, \
                 CAST(x AS Float64), arrayJoin(record.items::Array(String)), x::ENUM('a' = 1), x::Array(ENUM('b' = 2)) \
                 FROM ("
            ),
            "{out}"
        );
    }

    #[test]
    fn comments_and_a_trailing_semicolon_are_dropped() {
        let bare = rendered("SELECT * FROM orders");
        assert_eq!(rendered("SELECT * FROM orders -- FORMAT Vertical"), bare);
        assert_eq!(rendered("SELECT * /* SETTINGS x = 1 */ FROM orders;"), bare);
    }

    #[test]
    fn everyday_clickhouse_syntax_round_trips_around_the_mirage() {
        for sql in [
            "SELECT arrayMap(x -> x * 2, record.items::Array(Int64)) AS d FROM orders LIMIT 1 BY cursor",
            "SELECT count() FROM orders GROUP BY record.customer WITH TOTALS",
            "SELECT * FROM orders WHERE event_ts_ms > now() - INTERVAL 1 HOUR QUALIFY row_number() OVER (ORDER BY cursor) = 1",
            "SELECT toStartOfInterval(toDateTime(event_ts_ms / 1000), INTERVAL 5 MINUTE) AS t, count() FROM orders GROUP BY t ORDER BY t",
            "SELECT record.x AS `weird alias`, `record`.`y`, quantile(0.9)(record.price::Float64) FROM orders",
            "SELECT DISTINCT record.customer FROM orders WHERE record.x LIKE '%a%' AND record.y ILIKE 'b'",
        ] {
            let expected = sql.replace("FROM orders", &format!("FROM {}", mirage("orders", "orders")));
            assert_eq!(rendered(sql), format!("{expected} FORMAT JSONEachRow"));
        }
    }
}

mod rejects {
    use super::*;

    #[test]
    fn settings_at_any_nesting_level() {
        for sql in [
            "SELECT * FROM orders SETTINGS x = 1",
            "SELECT * FROM (SELECT * FROM orders SETTINGS x = 1) AS q",
            "(SELECT * FROM orders SETTINGS x = 1) UNION ALL SELECT * FROM customers",
            "SELECT * FROM orders WHERE 1 IN (SELECT 1 FROM orders SETTINGS x = 1)",
        ] {
            assert_eq!(refused(sql), GuardError::Settings, "{sql}");
        }
    }

    #[test]
    fn format_at_any_nesting_level() {
        assert_eq!(refused("SELECT * FROM orders FORMAT JSON"), GuardError::Format);
        assert_eq!(refused("SELECT * FROM (SELECT * FROM orders FORMAT JSON) AS q"), GuardError::Format);
        assert_eq!(refused("SELECT * FROM orders WHERE 1 IN (SELECT 1 FROM orders FORMAT JSON)"), GuardError::Format);
    }

    #[test]
    fn into_in_either_spelling() {
        assert_eq!(refused("SELECT * INTO t FROM orders"), GuardError::IntoOutfile);
        assert!(matches!(refused("SELECT * FROM orders INTO OUTFILE 'x'"), GuardError::Parse(_)));
    }

    #[test]
    fn anything_but_one_select() {
        for sql in [
            "INSERT INTO orders VALUES (1)",
            "EXPLAIN SELECT 1",
            "SHOW TABLES",
            "DESCRIBE orders",
            "VALUES (1)",
            "SELECT 1 UNION ALL TABLE records",
            "",
        ] {
            assert_eq!(refused(sql), GuardError::NotAQuery, "{sql}");
        }
        assert_eq!(refused("SELECT 1; SELECT 2"), GuardError::MultipleStatements);
        assert_eq!(refused("SELECT * FROM orders; DROP TABLE orders"), GuardError::MultipleStatements);
    }

    #[test]
    fn table_functions_by_name_wherever_they_hide() {
        for (sql, name) in [
            ("SELECT * FROM url('http://x', 'RawBLOB')", "url"),
            ("SELECT * FROM remote('h', 'db', 't')", "remote"),
            ("SELECT * FROM file('a.csv')", "file"),
            ("SELECT * FROM s3('http://b/x')", "s3"),
            ("SELECT * FROM merge('db', '^t')", "merge"),
            ("SELECT * FROM `url`('http://x')", "`url`"),
            ("SELECT * FROM orders CROSS JOIN numbers(3)", "numbers"),
            ("SELECT * FROM UNNEST([1, 2]) AS u", "UNNEST"),
            ("SELECT * FROM orders ARRAY JOIN arrayMap(x -> x, record.items) AS t", "arrayMap"),
            ("WITH t AS (SELECT * FROM url('http://x')) SELECT * FROM t", "url"),
            ("SELECT * FROM orders WHERE 1 = (SELECT count() FROM url('http://x'))", "url"),
        ] {
            assert_eq!(refused(sql), GuardError::TableFunction(name.into()), "{sql}");
        }
    }

    #[test]
    fn qualified_names_wherever_a_query_can_hide() {
        for (sql, name) in [
            ("SELECT * FROM db.orders", "db.orders"),
            ("SELECT * FROM system.tables", "system.tables"),
            ("SELECT * FROM `system`.`one`", "`system`.`one`"),
            ("SELECT * FROM orders JOIN system.one ON 1 = 1", "system.one"),
            ("SELECT * FROM orders UNION ALL SELECT * FROM system.tables", "system.tables"),
            ("SELECT cursor FROM orders EXCEPT SELECT 1 FROM system.one", "system.one"),
            ("SELECT cursor FROM orders INTERSECT SELECT 1 FROM system.one", "system.one"),
            ("SELECT (SELECT count() FROM system.tables) FROM orders", "system.tables"),
            ("SELECT * FROM orders WHERE EXISTS (SELECT 1 FROM system.one)", "system.one"),
            ("SELECT * FROM orders WHERE 1 IN (1, (SELECT 1 FROM system.one))", "system.one"),
            ("SELECT * FROM orders WHERE 1 = ANY (SELECT 1 FROM system.one)", "system.one"),
            ("SELECT count() OVER (PARTITION BY (SELECT 1 FROM system.one)) FROM orders", "system.one"),
            ("SELECT * FROM orders WINDOW w AS (PARTITION BY (SELECT 1 FROM system.one))", "system.one"),
            ("SELECT arrayMap(x -> (SELECT 1 FROM system.one), record.items) FROM orders", "system.one"),
            ("SELECT count() FROM orders HAVING 1 IN (SELECT 1 FROM system.one)", "system.one"),
            ("SELECT * FROM orders ORDER BY (SELECT 1 FROM system.one)", "system.one"),
            ("SELECT * FROM orders LIMIT (SELECT 1 FROM system.one)", "system.one"),
            ("WITH t AS (SELECT * FROM system.one) SELECT 1", "system.one"),
            (
                "SELECT * FROM orders WHERE cursor IN (SELECT cursor FROM orders WHERE 1 IN (SELECT 1 FROM system.one))",
                "system.one",
            ),
        ] {
            assert_eq!(refused(sql), GuardError::QualifiedName(name.into()), "{sql}");
        }
    }

    #[test]
    fn other_from_syntax_and_table_modifiers() {
        for sql in [
            "SELECT * FROM TABLE(orders)",
            "SELECT * FROM (orders JOIN customers ON 1 = 1)",
            "SELECT * FROM orders PIVOT (sum(a) FOR b IN (1))",
            "SELECT * FROM orders SAMPLE 0.1",
            "SELECT * FROM orders WITH (NOLOCK)",
            "SELECT * FROM orders WITH ORDINALITY",
        ] {
            assert!(matches!(refused(sql), GuardError::Forbidden(_)), "{sql}");
        }
    }

    #[test]
    fn prewhere_which_clickhouse_cannot_apply_to_a_subquery() {
        assert_eq!(refused("SELECT cursor FROM orders PREWHERE cursor > 1"), GuardError::Forbidden("PREWHERE".into()));
    }

    #[test]
    fn unknown_invisible_or_malformed_tables() {
        assert_eq!(refused("SELECT * FROM secrets"), GuardError::UnknownTable("secrets".into()));
        assert_eq!(refused("SELECT * FROM Orders"), GuardError::UnknownTable("Orders".into()));
        assert_eq!(refused("SELECT * FROM orders o JOIN o ON 1 = 1"), GuardError::UnknownTable("o".into()));
        assert_eq!(refused("SELECT * FROM (SELECT * FROM records)"), GuardError::UnknownTable("records".into()));
        assert!(matches!(refused("SELECT * FROM ordérs"), GuardError::Parse(_)));
    }

    #[test]
    fn a_cte_is_scoped_to_its_own_query() {
        assert_eq!(
            refused("SELECT * FROM (WITH t AS (SELECT 1) SELECT * FROM t) AS d, t"),
            GuardError::UnknownTable("t".into())
        );
        assert_eq!(
            refused("SELECT * FROM (WITH records AS (SELECT 1) SELECT * FROM records) AS d JOIN records ON 1 = 1"),
            GuardError::UnknownTable("records".into())
        );
    }

    #[test]
    fn a_cte_does_not_legitimise_its_own_name_inside_its_body() {
        for sql in [
            "WITH records AS (SELECT * FROM records) SELECT * FROM records",
            "WITH records AS (SELECT * FROM (SELECT * FROM records) AS x) SELECT * FROM records",
            "WITH t AS (SELECT 1), records AS (SELECT * FROM t JOIN records ON 1 = 1) SELECT * FROM records",
            "WITH records AS (SELECT * FROM orders WHERE 1 IN (SELECT 1 FROM records)) SELECT * FROM records",
        ] {
            assert_eq!(refused(sql), GuardError::UnknownTable("records".into()), "{sql}");
        }
        assert!(matches!(
            refused("WITH records AS (SELECT * FROM orders WHERE 1 IN (records)) SELECT * FROM records"),
            GuardError::Forbidden(_)
        ));
    }

    /// `WITH RECURSIVE` resolves a CTE's own name to the CTE — except when it is named after the
    /// physical table, which ClickHouse reads instead, so that one still goes through the
    /// allowlist. Recursion under any other name is untouched.
    #[test]
    fn a_recursive_cte_named_records_is_refused_while_other_names_still_recurse() {
        for sql in [
            "WITH RECURSIVE records AS (SELECT * FROM records) SELECT * FROM records",
            "WITH RECURSIVE t AS (SELECT 1 AS n), records AS (SELECT * FROM t JOIN records ON 1 = 1) SELECT * FROM records",
        ] {
            assert_eq!(refused(sql), GuardError::UnknownTable("records".into()), "{sql}");
        }
        let legitimate =
            "WITH RECURSIVE n AS (SELECT 1 AS i UNION ALL SELECT i + 1 FROM n WHERE i < 3) SELECT * FROM n";
        assert_eq!(rendered(legitimate), format!("{legitimate} FORMAT JSONEachRow"));
    }

    #[test]
    fn identifiers_in_in_lists_that_are_not_columns() {
        for sql in [
            "SELECT * FROM orders WHERE 1 IN (system.one)",
            "SELECT * FROM orders WHERE 1 IN ((system.one))",
            "SELECT * FROM orders WHERE 1 IN (`system`.one)",
            "SELECT (1, 2) IN ((system.one)) FROM orders",
            "SELECT * FROM orders WHERE 1 IN (space_station.records)",
            "SELECT * FROM orders WHERE 1 IN (x)",
            "SELECT * FROM orders WHERE 1 NOT IN (1, system.one)",
            "SELECT in(1, system.one) FROM orders",
            "SELECT * FROM orders WHERE `in`(1, system.one)",
            "SELECT * FROM orders WHERE globalNotIn(cursor, system.one)",
            "SELECT arrayMap(x -> 1 IN (system.one), record.items) FROM orders",
        ] {
            assert!(matches!(refused(sql), GuardError::Forbidden(_)), "{sql}");
        }
    }

    #[test]
    fn sql_over_64_kb_or_nested_past_the_parser_limit() {
        assert_eq!(refused(&format!("SELECT '{}'", "x".repeat(SQL_MAX))), GuardError::TooLong);
        let deep = refused(&format!("SELECT {}1{}", "(".repeat(60), ")".repeat(60)));
        assert!(deep.to_string().contains("nested too deeply"), "{deep}");
    }
}

mod bounds {
    use super::*;

    fn bounded(sql: &str, table: &str, from: Option<u64>, to: Option<u64>) -> String {
        planned(sql).render("acme", &BTreeMap::from([(table.into(), Bounds { from, to })]))
    }

    #[test]
    fn from_and_to_become_cursor_predicates() {
        let out = bounded("SELECT * FROM orders", "orders", Some(3), Some(9));
        assert!(out.contains("AND table_id = 'orders' AND cursor > 3 AND cursor <= 9) AS orders"), "{out}");
    }

    #[test]
    fn either_side_alone_or_neither() {
        assert!(
            bounded("SELECT * FROM orders", "orders", Some(3), None).contains("'orders' AND cursor > 3) AS orders")
        );
        assert!(
            bounded("SELECT * FROM orders", "orders", None, Some(9)).contains("'orders' AND cursor <= 9) AS orders")
        );
        assert!(bounded("SELECT * FROM orders", "orders", None, None).contains("'orders') AS orders"));
    }

    #[test]
    fn bounds_apply_per_table_and_unreferenced_ones_are_ignored() {
        let out = bounded("SELECT * FROM orders, customers", "customers", Some(1), None);
        assert!(
            out.contains("'orders') AS orders") && out.contains("'customers' AND cursor > 1) AS customers"),
            "{out}"
        );
        assert!(bounded("SELECT * FROM orders", "customers", Some(1), None).contains("'orders') AS orders"));
    }

    #[test]
    fn an_org_that_is_not_an_id_matches_nothing() {
        assert!(planned("SELECT * FROM orders").render("Acme'", &BTreeMap::new()).contains("WHERE org_id = '' AND"));
        assert!(valid_org("acme-1_x") && !valid_org("ab") && !valid_org("Acme") && !valid_org(&"a".repeat(51)));
    }
}

mod triggers {
    use super::*;

    #[test]
    fn where_is_spliced_in_parentheses_over_the_flush_range() {
        assert_eq!(
            trigger_sql("acme", "orders", Some("record.price::Float64 > 5"), 3, 9).unwrap(),
            "SELECT 1 FROM (SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM space_station.records \
             WHERE org_id = 'acme' AND table_id = 'orders' AND cursor > 3 AND cursor <= 9) AS orders \
             WHERE (record.price::Float64 > 5) LIMIT 1 FORMAT JSONEachRow"
        );
    }

    #[test]
    fn no_where_means_any_row() {
        assert_eq!(
            trigger_sql("acme", "orders", None, 3, 9).unwrap(),
            "SELECT 1 FROM (SELECT cursor, record_id, event_ts_ms, registered_ts_ms, metadata, record FROM space_station.records \
             WHERE org_id = 'acme' AND table_id = 'orders' AND cursor > 3 AND cursor <= 9) AS orders LIMIT 1 FORMAT JSONEachRow"
        );
    }

    #[test]
    fn a_subquery_in_where_may_read_another_org_table_unbounded() {
        let out =
            trigger_sql("acme", "orders", Some("record.customer IN (SELECT record.id FROM customers)"), 3, 9).unwrap();
        assert!(out.contains(&mirage("customers", "customers")), "{out}");
    }

    #[test]
    fn where_obeys_the_guard() {
        assert_eq!(
            trigger_sql("acme", "orders", Some("1 IN (SELECT 1 FROM system.one)"), 0, 1),
            Err(GuardError::QualifiedName("system.one".into()))
        );
        for where_ in ["1 IN (system.one)", "record.x IN (t)"] {
            assert!(
                matches!(trigger_sql("acme", "orders", Some(where_), 0, 1), Err(GuardError::Forbidden(_))),
                "{where_}"
            );
        }
        for where_ in ["1 = 1 LIMIT 5", "1 = 1 SETTINGS x = 1", "1 = 1; SELECT 2", ""] {
            assert!(matches!(trigger_sql("acme", "orders", Some(where_), 0, 1), Err(GuardError::Parse(_))), "{where_}");
        }
        assert_eq!(
            trigger_sql("acme", "orders", Some(&"1 = 1 OR ".repeat(SQL_MAX / 9 + 1)), 0, 1),
            Err(GuardError::TooLong)
        );
    }

    #[test]
    fn org_and_table_ids_are_checked_first() {
        assert!(matches!(trigger_sql("Acme", "orders", None, 0, 1), Err(GuardError::Forbidden(_))));
        assert_eq!(trigger_sql("acme", "orders x", None, 0, 1), Err(GuardError::UnknownTable("orders x".into())));
    }

    #[test]
    fn a_trigger_plan_lists_every_table_for_the_savers_visibility_check_and_renders_like_trigger_sql() {
        let where_ = "record.customer IN (SELECT record.id FROM customers)";
        let plan = trigger_plan("orders", Some(where_), &visible).unwrap();
        assert_eq!(plan.tables().iter().cloned().collect::<Vec<_>>(), ["customers", "orders"]);
        let bounds = BTreeMap::from([("orders".into(), Bounds { from: Some(3), to: Some(9) })]);
        assert_eq!(plan.render("acme", &bounds), trigger_sql("acme", "orders", Some(where_), 3, 9).unwrap());
        assert_eq!(
            trigger_plan("orders", Some("1 IN (SELECT 1 FROM secrets)"), &visible).err(),
            Some(GuardError::UnknownTable("secrets".into()))
        );
        assert_eq!(trigger_plan("secrets", None, &visible).err(), Some(GuardError::UnknownTable("secrets".into())));
    }
}

mod errors {
    use super::*;

    #[test]
    fn every_variant_has_a_snake_case_code_and_a_one_line_message() {
        let all = [
            GuardError::Parse("x".into()),
            GuardError::NotAQuery,
            GuardError::MultipleStatements,
            GuardError::Settings,
            GuardError::Format,
            GuardError::IntoOutfile,
            GuardError::TableFunction("url".into()),
            GuardError::QualifiedName("system.one".into()),
            GuardError::UnknownTable("t".into()),
            GuardError::Forbidden("x".into()),
            GuardError::TooLong,
        ];
        let codes: BTreeSet<&str> = all.iter().map(GuardError::code).collect();
        assert_eq!(codes.len(), all.len());
        for e in &all {
            assert!(e.code().bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}", e.code());
            let message = e.to_string();
            assert!(!message.is_empty() && !message.contains('\n'), "{message}");
        }
        assert_eq!(refused("SELECT * FROM orders INTO OUTFILE 'x'").to_string().lines().count(), 1);
    }
}

/// The rendered SQL is real ClickHouse and the row policy is the boundary: creates the real
/// `space_station.records` (the DDL of docs/ARCHITECTURE.md, IF NOT EXISTS), inserts rows under
/// two throwaway orgs, runs renders and probes as the admin, then — when the doc's `ss_query`
/// user answers (password `CLICKHOUSE_QUERY_PASSWORD`, default `ss_query`) — runs them again
/// through `SQL_org` under `readonly = 1` and reads the physical table to prove one org cannot
/// see the other even without the rewrite. Deletes its rows at the end. Skips when nothing
/// answers on `CLICKHOUSE_URL` (default `http://dev:dev@localhost:8123`).
#[test]
fn rendered_sql_runs_on_clickhouse_and_the_row_policy_holds() {
    let admin: reqwest::Url =
        std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://dev:dev@localhost:8123".into()).parse().unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::new();
    let run = |url: &reqwest::Url, params: &[(&str, &str)], sql: String| -> Result<String, String> {
        let mut url = url.clone();
        url.query_pairs_mut().extend_pairs(params);
        rt.block_on(async {
            let response = client.post(url).body(sql).send().await.map_err(|e| e.to_string())?;
            let ok = response.status().is_success();
            let text = response.text().await.map_err(|e| e.to_string())?;
            if ok { Ok(text) } else { Err(text) }
        })
    };
    if let Err(e) = run(&admin, &[], "SELECT 1".into()) {
        eprintln!("skipping: no ClickHouse at {admin}: {e}");
        return;
    }
    run(
        &admin,
        &[],
        "CREATE TABLE IF NOT EXISTS space_station.records (\
           org_id LowCardinality(String), table_id LowCardinality(String), cursor UInt64, record_id UUID, \
           event_ts_ms Int64, registered_ts_ms Int64, metadata JSON, record JSON) \
         ENGINE = MergeTree ORDER BY (org_id, table_id, event_ts_ms) SETTINGS non_replicated_deduplication_window = 100"
            .into(),
    )
    .unwrap();
    let (a, b) = (
        format!("guardtest-{}", uuid::Uuid::new_v4().simple()),
        format!("guardtest-{}", uuid::Uuid::new_v4().simple()),
    );
    let row = |org: &str, table: &str, cursor: u64, record: &str| {
        format!(
            "{{\"org_id\":\"{org}\",\"table_id\":\"{table}\",\"cursor\":{cursor},\"record_id\":\"{}\",\
             \"event_ts_ms\":{cursor},\"registered_ts_ms\":{cursor},\"metadata\":{{}},\"record\":{record}}}",
            uuid::Uuid::new_v4()
        )
    };
    run(
        &admin,
        &[],
        format!(
            "INSERT INTO space_station.records FORMAT JSONEachRow\n{}\n{}\n{}\n{}",
            row(&a, "orders", 1, r#"{"price":7.5,"customer":"ann","items":["a","b"]}"#),
            row(&a, "orders", 2, r#"{"price":3,"customer":"bob","items":["c"]}"#),
            row(&a, "customers", 1, r#"{"id":"ann","tier":"gold"}"#),
            row(&b, "orders", 1, r#"{"price":99,"customer":"eve"}"#),
        ),
    )
    .unwrap();

    let query = |sql: &str, bounds: BTreeMap<String, Bounds>| run(&admin, &[], planned(sql).render(&a, &bounds));
    assert_eq!(
        query(
            "SELECT record.customer::String AS c, record.price::Float64 AS p FROM orders ORDER BY cursor",
            BTreeMap::new()
        )
        .unwrap(),
        "{\"c\":\"ann\",\"p\":7.5}\n{\"c\":\"bob\",\"p\":3}\n"
    );
    assert_eq!(
        query(
            "SELECT o.record.customer::String AS c, c.record.tier::String AS tier FROM orders o \
             JOIN customers c ON c.record.id::String = o.record.customer::String",
            BTreeMap::from([("orders".into(), Bounds { from: Some(0), to: Some(1) })])
        )
        .unwrap(),
        "{\"c\":\"ann\",\"tier\":\"gold\"}\n"
    );
    assert_eq!(
        query(
            "WITH t AS (SELECT arrayJoin(record.items::Array(String)) AS i FROM orders) \
             SELECT i, toUInt8(count()) AS n FROM t GROUP BY i ORDER BY i",
            BTreeMap::new()
        )
        .unwrap(),
        "{\"i\":\"a\",\"n\":1}\n{\"i\":\"b\",\"n\":1}\n{\"i\":\"c\",\"n\":1}\n"
    );
    assert_eq!(
        query(
            "SELECT i FROM (SELECT record.items::Array(String) AS items FROM orders) ARRAY JOIN items AS i ORDER BY i",
            BTreeMap::new()
        )
        .unwrap(),
        "{\"i\":\"a\"}\n{\"i\":\"b\"}\n{\"i\":\"c\"}\n",
        "ARRAY JOIN over a JSON path needs the cast in a derived table"
    );
    assert_eq!(
        query(
            "WITH orders AS (SELECT cursor FROM orders WHERE cursor > 1) SELECT toUInt8(cursor) AS c FROM orders",
            BTreeMap::new()
        )
        .unwrap(),
        "{\"c\":2}\n",
        "a CTE naming itself reads the org table inside"
    );
    assert!(
        query("SELECT org_id, table_id FROM orders", BTreeMap::new()).is_err(),
        "the physical columns are not in the mirage"
    );
    let probe = |from, to| {
        run(&admin, &[], trigger_sql(&a, "orders", Some("record.price::Float64 > 5"), from, to).unwrap()).unwrap()
    };
    assert_eq!(probe(0, 2), "{\"1\":1}\n");
    assert_eq!(probe(1, 2), "");

    let mut user = admin.clone();
    user.set_username("ss_query").unwrap();
    user.set_password(Some(&std::env::var("CLICKHOUSE_QUERY_PASSWORD").unwrap_or_else(|_| "ss_query".into()))).unwrap();
    let physical = "SELECT DISTINCT org_id FROM space_station.records WHERE org_id LIKE 'guardtest-%' ORDER BY org_id FORMAT JSONEachRow";
    match run(&user, &[("SQL_org", &a)], "SELECT 1".into()) {
        Err(e) => eprintln!("skipping the row-policy proof: ss_query cannot log in: {e}"),
        Ok(_) => {
            let as_org = |org: &str, sql: String| run(&user, &[("SQL_org", org)], sql);
            let none = BTreeMap::new();
            assert_eq!(
                as_org(
                    &a,
                    planned("SELECT record.customer::String AS c FROM orders ORDER BY cursor").render(&a, &none)
                )
                .unwrap(),
                "{\"c\":\"ann\"}\n{\"c\":\"bob\"}\n",
                "the mirage runs under readonly with SQL_org as a URL parameter"
            );
            assert_eq!(
                as_org(&a, planned("SELECT cursor FROM orders ORDER BY cursor LIMIT 1").render(&a, &none)).unwrap(),
                "{\"cursor\":\"1\"}\n",
                "64-bit integers arrive quoted"
            );
            assert_eq!(as_org(&a, trigger_sql(&a, "orders", None, 0, 1).unwrap()).unwrap(), "{\"1\":1}\n");
            assert_eq!(
                as_org(&a, physical.into()).unwrap(),
                format!("{{\"org_id\":\"{a}\"}}\n"),
                "org A sees only itself"
            );
            assert_eq!(
                as_org(&b, physical.into()).unwrap(),
                format!("{{\"org_id\":\"{b}\"}}\n"),
                "org B sees only itself"
            );
            assert_eq!(run(&user, &[], physical.into()).unwrap(), "", "no SQL_org, no rows");
            assert!(run(&user, &[("readonly", "0")], "SELECT 1".into()).unwrap_err().contains("readonly"));
            assert!(run(&user, &[], "SELECT count() FROM system.query_log".into()).is_err(), "no system logs");
        }
    }
    run(&admin, &[], format!("DELETE FROM space_station.records WHERE org_id IN ('{a}', '{b}')")).unwrap();
}
