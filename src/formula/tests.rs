//! Unit tests for the formula engine.
#![cfg(test)]

use crate::entity::prelude::*;
use crate::formula::{AggregateFunction, FormulaExt, Node, compile_to_expr};
use crate::sea_query::PostgresQueryBuilder;
use crate::tests_cfg::{customer, dense_row, double_row, product, sale, sale_line};
use crate::{DbBackend, QueryTrait};

/// Render a single formula node to Postgres SQL.
fn render(node: &Node, root: &'static str) -> String {
    let expr = compile_to_expr(node, root.into()).unwrap();
    crate::sea_query::Query::select()
        .expr(expr)
        .to_string(PostgresQueryBuilder)
}

#[test]
fn scalar_arithmetic_net_price() {
    // net_price = quantity * unit_price - COALESCE(discount, 0)
    let net_price = Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap());

    let sql = render(&net_price, "sale_line");
    assert!(
        sql.contains(r#""sale_line"."quantity" * "sale_line"."unit_price""#),
        "got: {sql}"
    );
    assert!(
        sql.contains("COALESCE(\"sale_line\".\"discount\", 0)"),
        "got: {sql}"
    );
}

#[test]
fn to_one_related_product_cost() {
    // cost = quantity * product.cost  (product is a belongs_to of sale_line)
    let cost = Node::column(sale_line::Column::Quantity).mul(Node::related(
        sale_line::Relation::Product,
        Node::column(product::Column::Cost),
    ));

    let sql = render(&cost, "sale_line");
    assert!(sql.contains(r#""product"."cost""#), "got: {sql}");
    assert!(sql.contains(r#"FROM "product""#), "got: {sql}");
    assert!(
        sql.contains(r#""product"."id" = "sale_line"."product_id""#),
        "correlation missing: {sql}"
    );
}

#[test]
fn coalesce_wraps_aggregate() {
    // total_revenue = COALESCE(SUM(sales.balance), 0)
    let total = Node::coalesce([
        Node::aggregate(
            customer::Relation::Sale,
            AggregateFunction::Sum,
            Node::column(sale::Column::Balance),
        ),
        Node::int(0),
    ])
    .unwrap();

    let sql = render(&total, "customer");
    assert!(sql.starts_with("SELECT COALESCE("), "got: {sql}");
    assert!(sql.contains("SUM(\"sale\".\"balance\")"), "got: {sql}");
    assert!(
        sql.contains(r#""sale"."customer_id" = "customer"."id""#),
        "got: {sql}"
    );
}

#[test]
fn nested_correlated_aggregates_full_chain() {
    // final_price (sale_line) then SUM over lines then SUM over sales (customer)
    let net = Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap());
    let vat = net
        .clone()
        .mul(Node::column(sale_line::Column::VatRate))
        .div(Node::int(100));
    let final_price = net.clone().add(vat);

    let sale_total = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        final_price.clone(),
    );

    let customer_revenue =
        Node::aggregate(customer::Relation::Sale, AggregateFunction::Sum, sale_total);

    let sql = render(&customer_revenue, "customer");
    assert!(
        sql.contains(r#""sale_line"."sale_id" = "sale"."id""#),
        "lines->sale correlation missing: {sql}"
    );
    assert!(
        sql.contains(r#""sale"."customer_id" = "customer"."id""#),
        "sale->customer correlation missing: {sql}"
    );
}

#[test]
fn case_expression() {
    // CASE WHEN balance = 0 THEN 'PAID' WHEN balance > 1000 THEN 'LARGE'
    //      ELSE 'OPEN' END
    let status = Node::case(
        [
            (
                Node::column(sale::Column::Balance).eq(Node::int(0)),
                Node::str("PAID"),
            ),
            (
                Node::column(sale::Column::Balance).gt(Node::int(1000)),
                Node::str("LARGE"),
            ),
        ],
        Node::str("OPEN"),
    );
    let sql = render(&status, "sale");
    assert!(sql.contains("CASE WHEN"), "got: {sql}");
    assert!(
        sql.contains("'PAID'") && sql.contains("'LARGE'"),
        "got: {sql}"
    );
    assert!(sql.contains("ELSE 'OPEN'"), "got: {sql}");
}

#[test]
fn type_mismatch_is_rejected() {
    let bad = Node::column(sale_line::Column::Quantity).add(Node::str("oops"));
    let err = compile_to_expr(&bad, "sale_line".into()).unwrap_err();
    assert!(
        err.message.contains("cannot apply arithmetic operator `+`"),
        "{err}"
    );
}

#[test]
fn cross_table_without_hop_is_rejected() {
    let bad = Node::column(sale::Column::Status);
    let err = compile_to_expr(&bad, "sale_line".into()).unwrap_err();
    assert!(err.message.contains("not reachable"), "{err}");
}

#[test]
fn select_integration_formula() {
    let open = Node::column(sale::Column::Status).eq(Node::str("OPEN"));
    let outstanding = Node::aggregate_filtered(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        Node::column(sale::Column::Balance),
        Some(open),
    );

    let sql = customer::Entity::find()
        .formula(&outstanding, "outstanding")
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.contains(r#"FROM "customer""#), "got: {sql}");
    assert!(
        sql.contains(r#""sale"."customer_id" = "customer"."id""#),
        "got: {sql}"
    );
    assert!(sql.contains(r#"AS "outstanding""#), "alias missing: {sql}");
    println!("select integration: {sql}");
}

#[test]
fn select_integration_filter_and_order() {
    let open = Node::column(sale::Column::Status).eq(Node::str("OPEN"));
    let outstanding = Node::coalesce([
        Node::aggregate_filtered(
            customer::Relation::Sale,
            AggregateFunction::Sum,
            Node::column(sale::Column::Balance),
            Some(open),
        ),
        Node::int(0),
    ])
    .unwrap();

    let filtered = customer::Entity::find()
        .filter_formula(&outstanding.clone().gt(Node::int(1000)))
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();
    assert!(filtered.contains(r#"> 1000"#), "got: {filtered}");

    let ordered = customer::Entity::find()
        .order_by_formula(&outstanding, crate::Order::Desc)
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();
    assert!(ordered.contains(r#"ORDER BY"#), "got: {ordered}");
}

/// Formula builders for the ERP dependency-graph suite.
fn sale_line_net_price() -> Node {
    Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap())
}

fn sale_line_final_price() -> Node {
    let net = sale_line_net_price();
    let vat = net
        .clone()
        .mul(Node::column(sale_line::Column::VatRate))
        .div(Node::int(100));
    net.add(vat)
}

fn sale_line_cost() -> Node {
    Node::column(sale_line::Column::Quantity).mul(Node::related(
        sale_line::Relation::Product,
        Node::column(product::Column::Cost),
    ))
}

fn sale_total() -> Node {
    Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        sale_line_final_price(),
    )
}

fn sale_margin() -> Node {
    Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        sale_line_final_price(),
    )
    .sub(Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        sale_line_cost(),
    ))
}

#[test]
fn erp_dependency_graph_compiles() {
    // SaleLine level
    let net = sale_line_net_price();
    let final_price = sale_line_final_price();
    let cost = sale_line_cost();
    let margin = net.clone().sub(cost.clone());
    for f in [&net, &final_price, &cost, &margin] {
        compile_to_expr(f, "sale_line".into()).unwrap();
    }

    // Sale level
    let subtotal = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        net.clone(),
    );
    let vat = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        final_price.clone().sub(net.clone()),
    );
    let total = sale_total();
    let total_cost = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        cost.clone(),
    );
    let sale_margin = sale_margin();
    for f in [&subtotal, &vat, &total, &total_cost, &sale_margin] {
        compile_to_expr(f, "sale".into()).unwrap();
    }

    // Customer level — dependent aggregates must correlate at both levels.
    let total_revenue = Node::aggregate(customer::Relation::Sale, AggregateFunction::Sum, total);
    let total_margin = Node::aggregate(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        sale_margin,
    );
    for f in [&total_revenue, &total_margin] {
        compile_to_expr(f, "customer".into()).unwrap();
    }
    assert_eq!(
        compile_to_expr(&total_revenue, "customer".into()).is_ok(),
        true
    );
}

fn render_with(node: &Node, root: &'static str, backend: DbBackend) -> String {
    let expr = compile_to_expr(node, root.into()).unwrap();
    use crate::sea_query::{MysqlQueryBuilder, SqliteQueryBuilder};
    match backend {
        DbBackend::Postgres => crate::sea_query::Query::select()
            .expr(expr)
            .to_string(PostgresQueryBuilder),
        DbBackend::MySql => crate::sea_query::Query::select()
            .expr(expr)
            .to_string(MysqlQueryBuilder),
        DbBackend::Sqlite => crate::sea_query::Query::select()
            .expr(expr)
            .to_string(SqliteQueryBuilder),
    }
}

#[test]
fn correlated_aggregate_renders_on_all_backends() {
    let open = Node::column(sale::Column::Status).eq(Node::str("OPEN"));
    let outstanding = Node::aggregate_filtered(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        Node::column(sale::Column::Balance),
        Some(open),
    );
    for backend in [DbBackend::Postgres, DbBackend::MySql, DbBackend::Sqlite] {
        let sql = render_with(&outstanding, "customer", backend);
        assert!(
            sql.contains(r#""sale"."customer_id" = "customer"."id""#)
                || sql.contains(r#"`sale`.`customer_id` = `customer`.`id`"#),
            "no correlation for {backend:?}: {sql}"
        );
        assert!(
            sql.contains(r#"status"#),
            "no filter for {backend:?}: {sql}"
        );
        assert!(sql.contains("'OPEN'"), "bad literal for {backend:?}: {sql}");
    }
}

#[test]
fn scalar_formula_on_own_entity_select() {
    let net = Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap());
    let sql = sale_line::Entity::find()
        .formula(&net, "net_price")
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();
    assert!(sql.contains(r#"FROM "sale_line""#), "got: {sql}");
    assert!(sql.contains(r#"AS "net_price""#), "got: {sql}");
    assert!(
        sql.contains(r#""sale_line"."quantity" * "sale_line"."unit_price""#),
        "got: {sql}"
    );
}

#[test]
fn comparison_type_mismatch_is_rejected() {
    let bad = Node::column(sale_line::Column::Quantity).gt(Node::str("x"));
    let err = compile_to_expr(&bad, "sale_line".into()).unwrap_err();
    assert!(
        err.message.contains("cannot apply formula operator `>`"),
        "{err}"
    );
}

#[test]
fn boolean_and_with_non_boolean_is_rejected() {
    // quantity AND quantity  -- quantities are integers, not booleans
    let bad =
        Node::column(sale_line::Column::Quantity).and(Node::column(sale_line::Column::Quantity));
    let err = compile_to_expr(&bad, "sale_line".into()).unwrap_err();
    assert!(err.message.contains("AND/OR"), "{err}");
}

#[test]
fn case_with_non_boolean_condition_is_rejected() {
    let bad = Node::case(
        [(Node::column(sale_line::Column::Quantity), Node::int(1))],
        Node::int(0),
    );
    let err = compile_to_expr(&bad, "sale_line".into()).unwrap_err();
    assert!(err.message.contains("CASE WHEN"), "{err}");
}

#[test]
fn type_of_reports_result_type() {
    use crate::formula::FormulaType;
    // net_price is Decimal money
    let net = sale_line_net_price();
    assert_eq!(
        crate::formula::type_of(&net, "sale_line".into()).unwrap(),
        FormulaType::Decimal
    );
    // SUM(balance) is Decimal; COUNT is Integer
    let sum = Node::aggregate(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        Node::column(sale::Column::Balance),
    );
    assert_eq!(
        crate::formula::type_of(&sum, "customer".into()).unwrap(),
        FormulaType::Decimal
    );
    let count = Node::aggregate(
        customer::Relation::Sale,
        AggregateFunction::Count,
        Node::column(sale::Column::Id),
    );
    assert_eq!(
        crate::formula::type_of(&count, "customer".into()).unwrap(),
        FormulaType::Integer
    );
    // comparison yields Boolean
    let cmp = Node::column(sale::Column::Status).eq(Node::str("OPEN"));
    assert_eq!(
        crate::formula::type_of(&cmp, "sale".into()).unwrap(),
        FormulaType::Boolean
    );
    // invalid formulas still fail validation
    assert!(
        crate::formula::type_of(&Node::column(sale::Column::Status), "sale_line".into()).is_err()
    );
}

#[test]
fn overdue_formula_with_today() {
    // overdue = SUM(sales.balance WHERE balance > 0 AND due_date < today())
    let cond = Node::column(sale::Column::Balance)
        .gt(Node::int(0))
        .and(Node::column(sale::Column::DueDate).lt(Node::today()));
    let overdue = Node::aggregate_filtered(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        Node::column(sale::Column::Balance),
        Some(cond),
    );
    let sql = render(&overdue, "customer");
    assert!(sql.contains("CURRENT_DATE"), "got: {sql}");
    assert!(sql.contains("AND"), "got: {sql}");
    assert!(
        sql.contains(r#""sale"."customer_id" = "customer"."id""#),
        "correlation missing: {sql}"
    );
    // today() is typed Date; comparing to the date column is allowed
    let t = crate::formula::type_of(
        &Node::column(sale::Column::DueDate).lt(Node::today()),
        "sale".into(),
    )
    .unwrap();
    assert_eq!(t, crate::formula::FormulaType::Boolean);
}

#[test]
fn parser_scalar_net_price() {
    use crate::formula::parse;
    // net_price = quantity * unit_price - COALESCE(discount, 0)
    let node = parse::<sale_line::Entity>("quantity * unit_price - COALESCE(discount, 0)").unwrap();
    let sql = render(&node, "sale_line");
    assert!(
        sql.contains(r#""sale_line"."quantity" * "sale_line"."unit_price""#),
        "got: {sql}"
    );
    assert!(
        sql.contains("COALESCE(\"sale_line\".\"discount\", 0)"),
        "got: {sql}"
    );
}

#[test]
fn parser_rejects_unknown_column_and_function() {
    use crate::formula::parse;
    let err = parse::<sale_line::Entity>("quantity * not_a_column").unwrap_err();
    assert!(err.message.contains("unknown column"), "{err}");
    let err = parse::<sale_line::Entity>("quantity * DROP_TABLE()").unwrap_err();
    assert!(err.message.contains("unknown function"), "{err}");
}

#[test]
fn parser_boolean_comparison() {
    use crate::formula::parse;
    let node = parse::<sale::Entity>("balance > 0 AND status = 'OPEN'").unwrap();
    let sql = render(&node, "sale");
    assert!(sql.contains(r#""sale"."balance" > 0"#), "got: {sql}");
    assert!(sql.contains(r#""sale"."status" = 'OPEN'"#), "got: {sql}");
    assert!(sql.contains("AND"), "got: {sql}");
}

#[test]
fn today_renders_on_all_backends() {
    let t = Node::today();
    for backend in [DbBackend::Postgres, DbBackend::MySql, DbBackend::Sqlite] {
        let sql = render_with(&t, "sale", backend);
        assert!(sql.contains("CURRENT_DATE"), "{backend:?}: {sql}");
    }
}

#[test]
fn prelude_exports_parse_and_type_of() {
    use crate::formula::prelude::*;
    // parse resolves a scalar against the entity's columns
    let n = parse::<sale::Entity>("balance > 0 AND status = 'OPEN'").unwrap();
    // type_of validates it is Boolean without lowering to SQL
    assert_eq!(type_of(&n, "sale".into()).unwrap(), FormulaType::Boolean);
}

#[test]
fn nested_aggregate_renders_on_all_backends() {
    // customer revenue = SUM over sales of SUM over lines(final_price)
    let net = Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap());
    let sale_total = Node::aggregate(sale::Relation::SaleLine, AggregateFunction::Sum, net);
    let customer_revenue =
        Node::aggregate(customer::Relation::Sale, AggregateFunction::Sum, sale_total);
    for backend in [DbBackend::Postgres, DbBackend::MySql, DbBackend::Sqlite] {
        let sql = render_with(&customer_revenue, "customer", backend);
        assert!(
            sql.contains(r#""sale_line"."sale_id" = "sale"."id""#)
                || sql.contains(r#"`sale_line`.`sale_id` = `sale`.`id`"#),
            "lines->sale correlation missing on {backend:?}: {sql}"
        );
        assert!(
            sql.contains(r#""sale"."customer_id" = "customer"."id""#)
                || sql.contains(r#"`sale`.`customer_id` = `customer`.`id`"#),
            "sale->customer correlation missing on {backend:?}: {sql}"
        );
    }
}

#[test]
fn scalar_formula_order_by() {
    // scalar net_price used to order SaleLine rows
    let net =
        Node::column(sale_line::Column::Quantity).mul(Node::column(sale_line::Column::UnitPrice));
    let sql = sale_line::Entity::find()
        .order_by_formula(&net, crate::Order::Desc)
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();
    assert!(
        sql.contains(r#"ORDER BY "sale_line"."quantity" * "sale_line"."unit_price" DESC"#),
        "got: {sql}"
    );
}

#[test]
fn operator_function_and_cast_coverage() {
    use crate::formula::FormulaType;
    // sale-scope expressions over sale columns
    let balance = Node::column(sale::Column::Balance); // Decimal
    let sale_exprs = [
        Node::nullif(balance.clone(), Node::int(0)),
        balance.clone().abs(),
        Node::column(sale::Column::Status).lower(),
        Node::column(sale::Column::Status).upper(),
        Node::column(sale::Column::Status).length(),
        Node::cast(balance.clone(), FormulaType::Integer),
        balance.clone().neg(),
        balance.clone().is_null(),
        balance.clone().is_not_null(),
        Node::boolean(true).not(),
        Node::coalesce([balance.clone(), Node::int(0)]).unwrap(),
        Node::today(),
    ];
    for n in sale_exprs {
        compile_to_expr(&n, "sale".into()).unwrap();
    }
    // sale_line discount is nullable; IS NULL allowed on it
    let disc = Node::column(sale_line::Column::Discount);
    compile_to_expr(&disc.clone().is_null(), "sale_line".into()).unwrap();
    compile_to_expr(&disc.is_not_null(), "sale_line".into()).unwrap();
}

#[test]
fn aggregate_variety() {
    let qty = Node::column(sale_line::Column::Quantity); // Integer
    for (fun, expect_ty) in [
        (AggregateFunction::Sum, crate::formula::FormulaType::Decimal),
        (AggregateFunction::Avg, crate::formula::FormulaType::Decimal),
        (AggregateFunction::Min, crate::formula::FormulaType::Integer),
        (AggregateFunction::Max, crate::formula::FormulaType::Integer),
        (
            AggregateFunction::Count,
            crate::formula::FormulaType::Integer,
        ),
    ] {
        let n = Node::aggregate(sale::Relation::SaleLine, fun, qty.clone());
        let sql = render(&n, "sale");
        assert!(sql.contains(&format!("{}(", fun)), "got: {sql}");
        assert_eq!(
            crate::formula::type_of(&n, "sale".into()).unwrap(),
            expect_ty,
            "{fun}"
        );
    }
}

#[test]
fn stacked_formulas_project() {
    let a = Node::column(customer::Column::Id).add(Node::int(1));
    let b = Node::column(customer::Column::Id).mul(Node::int(2));
    let sql = customer::Entity::find()
        .formula(&a, "a")
        .unwrap()
        .formula(&b, "b")
        .unwrap()
        .build(DbBackend::Postgres)
        .to_string();
    assert!(sql.contains(r#"AS "a""#), "got: {sql}");
    assert!(sql.contains(r#"AS "b""#), "got: {sql}");
    // both formulas reference the customer root scope
    assert!(sql.contains(r#""customer"."id""#), "got: {sql}");
}

/// Auto-inclusion of computed fields (formulas) on `Entity::find()` and
/// `Entity::find_by_id()`. `double_row` declares `value_doubled = value * 2`
/// via `#[sea_orm(computed_fields = "computed_fields")]`, so the formula column
/// must be projected automatically without any manual `.formula(..)` call.
#[test]
fn auto_include_computed_field_in_find() {
    let sql = double_row::Entity::find()
        .build(crate::DbBackend::Postgres)
        .to_string();
    assert!(
        sql.starts_with(r#"SELECT "double_row"."id", "double_row"."value","#),
        "base columns must come first, got: {sql}"
    );
    assert!(
        sql.contains(r#"AS "value_doubled""#),
        "computed field not auto-included in find(): {sql}"
    );
}

#[test]
fn auto_include_computed_field_in_find_by_id() {
    let sql = double_row::Entity::find_by_id(7)
        .build(crate::DbBackend::Postgres)
        .to_string();
    assert!(
        sql.contains(r#"AS "value_doubled""#),
        "computed field not auto-included in find_by_id(): {sql}"
    );
    assert!(
        sql.contains(r#""double_row"."id" = 7"#),
        "pk filter still applied, got: {sql}"
    );
}

/// Dense `#[sea_orm::model]` (ModelEx) entities also auto-include computed
/// fields on `find()` / `find_by_id()`. `dense_row` declares
/// `value_tripled = value * 3` via `computed_fields = "computed_fields"`.
#[test]
fn auto_include_computed_field_in_dense_find() {
    let sql = dense_row::Entity::find()
        .build(crate::DbBackend::Postgres)
        .to_string();
    assert!(
        sql.starts_with(r#"SELECT "dense_row"."id", "dense_row"."value","#),
        "base columns must come first, got: {sql}"
    );
    assert!(
        sql.contains(r#"AS "value_tripled""#),
        "computed field not auto-included in dense find(): {sql}"
    );
}

#[test]
fn auto_include_computed_field_in_dense_find_by_id() {
    let sql = dense_row::Entity::find_by_id(3)
        .build(crate::DbBackend::Postgres)
        .to_string();
    assert!(
        sql.contains(r#"AS "value_tripled""#),
        "computed field not auto-included in dense find_by_id(): {sql}"
    );
    assert!(
        sql.contains(r#""dense_row"."id" = 3"#),
        "pk filter still applied, got: {sql}"
    );
}

/// End-to-end acceptance through SeaORM's public read path: run
/// `double_row::Entity::find()` against a mock connection and hydrate the row
/// (including the auto-included computed column) into a typed `FromQueryResult`
/// view. This exercises the real query -> `FromQueryResult` mapping boundary the
/// feature is consumed through, not just the generated SQL text.
#[cfg(feature = "mock")]
mod auto_include_acceptance {
    use crate as sea_orm;
    use crate::entity::prelude::*;
    use crate::tests_cfg::double_row;

    #[derive(Debug, PartialEq, crate::FromQueryResult)]
    struct DoubleRowView {
        id: i32,
        value: i32,
        value_doubled: i32,
    }

    #[tokio::test]
    async fn find_hydrates_auto_included_computed_field() {
        use crate::{DbBackend, MockDatabase, Value};

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "id" => Into::<Value>::into(1i32),
                "value" => Into::<Value>::into(5i32),
                "value_doubled" => Into::<Value>::into(10i32),
            }]])
            .into_connection();

        let rows: Vec<DoubleRowView> = double_row::Entity::find()
            .into_model::<DoubleRowView>()
            .all(&db)
            .await
            .expect("load double_row with computed field");

        assert_eq!(
            rows,
            [DoubleRowView {
                id: 1,
                value: 5,
                value_doubled: 10,
            }],
            "auto-included computed field must be readable through FromQueryResult"
        );
    }

    #[tokio::test]
    async fn find_by_id_hydrates_auto_included_computed_field() {
        use crate::{DbBackend, MockDatabase, Value};

        let db = MockDatabase::new(DbBackend::Postgres)
            .append_query_results([[maplit::btreemap! {
                "id" => Into::<Value>::into(7i32),
                "value" => Into::<Value>::into(6i32),
                "value_doubled" => Into::<Value>::into(12i32),
            }]])
            .into_connection();

        let row: Option<DoubleRowView> = double_row::Entity::find_by_id(7)
            .into_model::<DoubleRowView>()
            .one(&db)
            .await
            .expect("load double_row by id with computed field");

        assert_eq!(
            row,
            Some(DoubleRowView {
                id: 7,
                value: 6,
                value_doubled: 12,
            }),
            "find_by_id must hydrate the auto-included computed field"
        );
    }
}
