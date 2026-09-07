//! Live PostgreSQL acceptance tests for the formula subsystem.
//!
//! These tests verify the engine end-to-end against a real database:
//!   * the mandatory customer-specific pricing scenario (margin 50/30/15 for
//!     different unit prices of the same product), and
//!   * per-customer revenue correlation (revenue 100/80/65), proving that the
//!     generated correlated sub-queries do **not** aggregate across customers.
//!
//! They only run when the `sqlx-postgres` feature is enabled and `DATABASE_URL`
//! points at a PostgreSQL server.

#![cfg(feature = "sqlx-postgres")]

use sea_orm::{
    ConnectionTrait, Database, DbBackend, Statement,
    entity::prelude::*,
    formula::{AggregateFunction, Node, compile_to_expr},
    sea_query::{Expr, PostgresQueryBuilder, Query},
    tests_cfg::{customer, product, sale, sale_line},
};
use std::collections::BTreeMap;
use std::str::FromStr;

fn net_price() -> Node {
    Node::column(sale_line::Column::Quantity)
        .mul(Node::column(sale_line::Column::UnitPrice))
        .sub(Node::coalesce([Node::column(sale_line::Column::Discount), Node::int(0)]).unwrap())
}

fn final_price() -> Node {
    let net = net_price();
    let vat = net
        .clone()
        .mul(Node::column(sale_line::Column::VatRate))
        .div(Node::int(100));
    net.add(vat)
}

fn line_cost() -> Node {
    Node::column(sale_line::Column::Quantity).mul(Node::related(
        sale_line::Relation::Product,
        Node::column(product::Column::Cost),
    ))
}

fn customer_margin() -> Node {
    let sale_total = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        final_price(),
    );
    let sale_cost = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        line_cost(),
    );
    Node::aggregate(
        customer::Relation::Sale,
        AggregateFunction::Sum,
        sale_total.sub(sale_cost),
    )
}

async fn run_money_query(
    db: &sea_orm::DatabaseConnection,
    expr: sea_orm::sea_query::Expr,
    alias: &'static str,
) -> BTreeMap<i32, Decimal> {
    let sql = Query::select()
        .expr_as(Expr::col(("customer", "id")), "id")
        .expr_as(expr, alias)
        .from("customer")
        .to_owned()
        .to_string(PostgresQueryBuilder);
    let rows = db
        .query_all_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .expect("query");
    let mut out = BTreeMap::new();
    for row in &rows {
        let id: i32 = row.try_get("", "id").expect("id");
        let m: Decimal = row.try_get("", alias).expect("value");
        out.insert(id, m.normalize());
    }
    out
}

#[tokio::test]
async fn erp_customer_specific_pricing_and_revenue() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        eprintln!("skipping: DATABASE_URL is not postgres");
        return;
    }
    let db = Database::connect(&url).await.expect("connect postgres");

    for sql in [
        "DROP TABLE IF EXISTS sale_line, sale, erp_order, customer, product",
        "CREATE TABLE customer (id INT PRIMARY KEY, name TEXT)",
        "CREATE TABLE product (id INT PRIMARY KEY, name TEXT, cost NUMERIC(12,2))",
        "CREATE TABLE erp_order (id INT PRIMARY KEY, customer_id INT)",
        "CREATE TABLE sale (id INT PRIMARY KEY, customer_id INT, order_id INT, status TEXT, balance NUMERIC(12,2), due_date DATE)",
        "CREATE TABLE sale_line (id INT PRIMARY KEY, sale_id INT, product_id INT, quantity INT, unit_price NUMERIC(12,2), discount NUMERIC(12,2), vat_rate NUMERIC(12,2))",
        "INSERT INTO product VALUES (1, 'A', 5.00)",
        "INSERT INTO customer VALUES (1,'Customer A'),(2,'Customer B'),(3,'Customer C')",
        "INSERT INTO erp_order VALUES (1,1),(2,2),(3,3)",
        "INSERT INTO sale VALUES (1,1,1,'OPEN',0.00,'2024-01-01'),(2,2,2,'OPEN',0.00,'2024-01-01'),(3,3,3,'OPEN',0.00,'2024-01-01')",
        "INSERT INTO sale_line VALUES (1,1,1,10,10.00,NULL,0),(2,2,1,10,8.00,NULL,0),(3,3,1,10,6.50,NULL,0)",
    ] {
        db.execute_unprepared(sql)
            .await
            .expect(&format!("sql: {sql}"));
    }

    // margin = SUM over sales of (SUM lines final_price - SUM lines quantity*cost)
    let margin = compile_to_expr(&customer_margin(), "customer".into()).unwrap();
    let margins = run_money_query(&db, margin, "margin").await;
    // Customer-specific pricing (same product, different unit prices)
    for (id, expect) in [(1, "50.00"), (2, "30.00"), (3, "15.00")] {
        let got = margins.get(&id).copied();
        let exp = Decimal::from_str(expect).unwrap();
        assert_eq!(got, Some(exp), "customer {id} margin");
    }

    // total_revenue = SUM over sales of SUM over lines(final_price), correlated per customer.
    let sale_total = Node::aggregate(
        sale::Relation::SaleLine,
        AggregateFunction::Sum,
        final_price(),
    );
    let revenue = compile_to_expr(
        &Node::aggregate(customer::Relation::Sale, AggregateFunction::Sum, sale_total),
        "customer".into(),
    )
    .unwrap();
    let revs = run_money_query(&db, revenue, "revenue").await;
    // revenue must not aggregate across customers.
    for (id, expect) in [(1, "100"), (2, "80"), (3, "65")] {
        let exp = Decimal::from_str(expect).unwrap();
        assert_eq!(revs.get(&id).copied(), Some(exp), "customer {id} revenue");
    }
}
