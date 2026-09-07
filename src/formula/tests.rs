//! Unit tests for the formula engine.
#![cfg(test)]

use crate::entity::prelude::*;
use crate::formula::{AggregateFunction, FormulaExt, Node, compile_to_expr};
use crate::sea_query::PostgresQueryBuilder;
use crate::tests_cfg::{customer, product, sale, sale_line};
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
