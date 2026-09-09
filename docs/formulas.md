# Formulas & Computed Fields

This document describes the first-class, type-safe **Formula / computed-field**
subsystem added to this SeaORM fork, inspired by Hibernate's `@Formula` but
designed for Rust and SeaORM's strongly typed entity / relationship system.

## Goals

A formula is a *query-time* computed expression that is expressed in Rust (or,
later, translated from a descriptor by Ferris CMS), type-checked, and compiled
into safe SeaQuery SQL. Formulas never accept arbitrary SQL text: the compiler
only emits SQL from validated AST nodes.

Two concepts are deliberately kept distinct (see below):

* **Query formula** — computed on every `SELECT` (e.g. `SUM(customer.sales.total)`),
  which depends on other rows/tables.
* **Generated column** — a schema-level expression stored/calculated by the
  database as part of a table.

Both can share the same expression AST.

## Layering

The formula layer reuses the existing SeaORM → SeaQuery → SQL pipeline. It does
**not** introduce a parallel SQL builder.

```text
Formula AST (Node)
      │  validate + type-check (operand / expected / actual types)
      ▼
relationship resolution (SeaORM Relation metadata → correlation columns)
      ▼
correlated SeaQuery expressions (scalar sub-queries / aggregates)
      ▼
backend-specific SQL (Postgres, MySQL, SQLite, …)
```

Relationship correlation is derived automatically from the generated
`Relation` metadata: an aggregate over a `has_many` collection, or a fetch of a
`belongs_to` / `has_one` row, becomes a **correlated sub-query** so that a query
over thousands of customers never degenerates into N+1 round-trips.

## Public API

The public module is `sea_orm::formula`. Key items:

| Item | Meaning |
|------|---------|
| `formula::Node` | A typed formula expression AST (public, dynamic friendly). |
| `formula::FormulaType` | Strongly typed result type (`Boolean`, `Integer`, `Decimal`, `Float`, `String`, `Date`, `DateTime`, `Uuid`, `Json`, `Null`). |
| `formula::AggregateFunction` | `Sum`, `Avg`, `Min`, `Max`, `Count`. |
| `formula::FormulaExt` | Adds `.formula(..)`, `.filter_formula(..)`, `.order_by_formula(..)` to `Select<E>`. |
| `formula::compile_to_expr(node, root)` | Public compile entry point used by higher-level systems. |
| `formula::type_of(node, root)` | Public validation entry point returning the checked result `FormulaType`. |
| `formula::parse::<E>(str)` | Safe textual parser: resolves a scalar formula string against the columns of entity `E`. |
| `formula::FormulaError` | Rich error identifying expression, operand, expected/actual types. |

### Building formulas

Columns are referenced through their real SeaORM `Column` types, so each leaf
carries its owning table *and* its SQL type. Relationship traversal is explicit
through a relation variant of the entity currently in scope.

```rust
use sea_orm::formula::prelude::*;

// net_price = quantity * unit_price - COALESCE(discount, 0)
let net_price = Node::column(sale_line::Column::Quantity)
    .mul(Node::column(sale_line::Column::UnitPrice))
    .sub(Node::coalesce([
        Node::column(sale_line::Column::Discount),
        Node::int(0),
    ])?);
```

Aggregates and filtered aggregates over a collection:

```rust
// outstanding = SUM(sales.balance WHERE sales.status = 'OPEN')
let open = Node::column(sale::Column::Status).eq(Node::str("OPEN"));
let outstanding = Node::aggregate_filtered(
    customer::Relation::Sale,
    AggregateFunction::Sum,
    Node::column(sale::Column::Balance),
    Some(open),
);
```

`customer::Relation::Sale` is the generated `has_many` relation of `Customer` →
`Sale`; the compiler reads its `RelationDef` to discover the correlation columns
(`sale.customer_id = customer.id`).

### Using a formula in a query

```rust
let rows = Customer::find()
    .formula(&outstanding, "outstanding")?   // project
    .all(db)
    .await?;
```

Filtering and ordering by a formula (the formula is repeated in `WHERE` /
`ORDER BY`, which SQL requires):

```rust
Customer::find()
    .filter_formula(&outstanding.clone().gt(Node::int(1000)))?   // > 1000
    .order_by_formula(&outstanding, Order::Desc)?                 // top customers
```

### Automatic inclusion in `find()` / `find_by_id()` (Hibernate `@Formula`)

An entity can declare computed fields that are **automatically projected in
every `SELECT`** produced by `Entity::find()` / `Entity::find_by_id()`, so the
caller does not have to call `.formula(..)` on each query. The alias is the
column the computed value appears under in the result row; read it back with a
partial model / custom `FromQueryResult` (SeaORM's existing hydration paths).

On a classic `#[derive(DeriveEntityModel)]` entity, opt in with the
`computed_fields` container attribute pointing at a function that returns
`Vec<sea_orm::formula::ComputedField>`:

```rust,ignore
use sea_orm::formula::{ComputedField, Node};

fn computed_fields() -> Vec<ComputedField> {
    let doubled = Node::column(Column::Value).mul(Node::int(2));
    vec![ComputedField::for_entity::<Entity>("value_doubled", &doubled).unwrap()]
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "double_row", computed_fields = "computed_fields")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub value: i32,
}
```

Then `double_row::Entity::find()` emits:

```sql
SELECT "double_row"."id", "double_row"."value",
       "double_row"."value" * 2 AS "value_doubled"
FROM "double_row"
```

`find_by_id(...)` behaves the same (the formula columns are added on top of the
primary-key filter). Validation (reachability + type-checking) happens once when
the field is declared, so `find()` stays infallible. Related rows are still
hydrated through the existing eager-load APIs (`find_with_related`, partial
models), so this stays a "SELECT carries the columns" change: `find()` /
`find_by_id()` continue to return `Vec<Model>` / `Option<Model>`.

The opt-in works for both the classic `#[derive(DeriveEntityModel)]` form and
the dense `#[sea_orm::model]` (`ModelEx`) form. On the dense form the computed
field is carried in the same `#[sea_orm(...)]` attribute as the `ModelEx`
wiring, e.g.:

```rust,ignore
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "customer", computed_fields = "computed_fields")]
pub struct Model { /* ... */ }
```

## Relationship traversal and correlation

Because every column reference is fully qualified and every hop stores its own
source/target correlation, formulas compose:

```text
Customer.total_revenue = SUM(sales.total)
Sale.total             = SUM(lines.final_price)   // lines = has_many
SaleLine.final_price   = net_price + vat_amount    // scalar, own row
```

compiles to a single, correctly correlated statement (no N+1):

```sql
SELECT (SELECT SUM(
          (SELECT SUM( ((qty * unit_price - COALESCE(discount,0))
                        + ((qty*unit_price - COALESCE(discount,0)) * vat_rate / 100))
            FROM sale_line
            WHERE sale_line.sale_id = sale.id))
        FROM sale
        WHERE sale.customer_id = customer.id)
FROM customer ...
```

## ERP example domain

The `tests_cfg` module ships an ERP domain used by the tests and examples:

```text
Customer  ──< has_many >── Sale  ──< has_many >── SaleLine ──< belongs_to >── Product
     └──< has_many >── Order ──< has_many >── Sale
```

`SaleLine` stores `quantity`, `unit_price`, `discount`, `vat_rate` and the
`product_id`. `Product` stores `cost` (an exact `Decimal`). Because different
customers can buy the same product at different prices, margin is always
computed from the *line* price, never `product.sale_price - product.cost`.

## Type safety

Every node carries a [`FormulaType`](crate::formula::FormulaType). Arithmetic
requires numeric operands, comparisons require comparable types, boolean
operators require booleans, and `COALESCE`/`CASE` require compatible branch
types. Errors identify the offending operand and the expected vs. actual type:

```
cannot apply arithmetic operator `*` to operands of type `integer` and `string`
```

Referencing a column that is not reachable in the current scope (i.e. without a
relationship hop) is also rejected:

```
column `sale.status` is not reachable from the current scope `sale_line`:
it must be accessed through a relationship hop
```

## Generated columns vs query formulas

* A **query formula** is this subsystem's focus: it is projected/filtered/ordered
  on every query.
* A **generated column** belongs in the schema layer (`ColumnDef`) and is not
  implemented here. The shared AST is what would let both share a definition.

Materialization of formulas into stored columns is deliberately **not**
implemented yet, but the public AST leaves room to add `GeneratedColumn` /
`Materialized` variants later without changing the expression DSL.

## Testing

The engine ships with unit tests covering scalar arithmetic, `COALESCE` /
`NULLIF`, `CASE`, to-one traversal, correlated & filtered aggregates, nested
(dependent) aggregates, and `Select` formula/filter/order integration, asserting
the emitted correlated SQL across Postgres/MySQL/SQLite builders.

A live PostgreSQL integration test (`tests/formula_live.rs`, feature
`sqlx-postgres`) verifies the engine end-to-end against a real database. It
recreates the mandatory customer-specific pricing scenario (one product with
`cost = 5.00` sold to three customers at `10.00` / `8.00` / `6.50`, quantity
`10`) and asserts per-customer **margin** `50.00 / 30.00 / 15.00`, as well as
per-customer **revenue** `100 / 80 / 65` — confirming that the generated
correlated sub-queries stay per-customer and never aggregate across customers.
Run it with:

```sh
DATABASE_URL="postgres://sea:sea@localhost:5432/sea" \
  cargo test --features sqlx-postgres,runtime-tokio-rustls,tests-cfg \
  --test formula_live
```

Further live scenarios (multiple products, filtered aggregates such as
`outstanding` / `overdue`, and COUNT) and the 10/100/1000/10000-customer
no-N+1 benchmark are natural follow-ups to the same harness.

## Notes / not yet implemented

* A safe **scalar** textual parser (`formula::parse::<E>(str)`) is shipped: it
  resolves arithmetic / comparison / boolean / `COALESCE`-style expressions
  against the columns of a single entity. Relationship aggregates
  (`SUM(sales.total)`) are still expressed with the typed `Node` builder (so the
  relationship is validated against real SeaORM metadata) rather than parsed.
* Backend-specific optimisations (e.g. `LATERAL` joins on Postgres) are not yet
  emitted; correlated sub-queries are used everywhere.
* The live PostgreSQL integration test covers the pricing / revenue scenarios;
  additional live scenarios (multi-product, filtered aggregates, COUNT) and the
  10/100/1000/10000-customer no-N+1 benchmark are follow-ups.
* Materialised / generated-column storage of formulas is not yet implemented
  (see above).
