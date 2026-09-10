#![allow(unused_imports, dead_code)]

//! Tests for database-generated columns.
//!
//! `generated` is ORM lifecycle metadata: the database owns the value, so the
//! column is omitted from the corresponding `INSERT` / `UPDATE` statements and
//! its value is refreshed from the database afterwards.
//!
//! `column_definition` / `generated_expression` are schema-generation metadata:
//! they describe how the column is created.

pub mod common;

use sea_orm::entity::prelude::*;
use sea_orm::{DbBackend, Insert, IntoActiveModel, QueryTrait, Schema, Set, Transaction};

mod customer {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "customer")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub first_name: Option<String>,
        pub last_name: Option<String>,
        /// Database-first: SeaORM knows the DB owns the value but this entity
        /// does not describe how the column is created.
        #[sea_orm(generated = "always")]
        pub full_name: String,
        /// Entity-first (portable expression).
        #[sea_orm(
            generated = "insert",
            generated_expression = "first_name || ' ' || last_name"
        )]
        pub name_on_insert: String,
        /// Entity-first (raw, database-specific DDL).
        #[sea_orm(
            generated = "update",
            column_definition = "GENERATED ALWAYS AS (first_name) STORED"
        )]
        pub name_on_update: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod raw_definition {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "raw_definition")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        // `column_definition` alone is DDL only and must not make the column
        // database-generated for the ORM.
        #[sea_orm(column_definition = "COLLATE NOCASE")]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod sqlserver_style {
    use sea_orm::entity::prelude::*;

    // The motivating Hibernate/JPA use case, expressed with the raw
    // `column_definition` escape hatch. SeaORM does not parse the string; it is
    // emitted verbatim, so it can target any dialect (here SQL Server computed
    // columns). The ORM side is still fully generated-column aware.
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "customer")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub first_name: Option<String>,
        pub middle_name1: Option<String>,
        pub last_name: Option<String>,
        #[sea_orm(
            generated = "always",
            column_definition = "AS CONCAT(
                COALESCE(first_name, ''),
                COALESCE(' ' + middle_name1, ''),
                COALESCE(' ' + last_name, '')
            )"
        )]
        pub full_name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[test]
fn sqlserver_style_computed_column_ddl_and_orm() {
    use sea_orm::ColumnTrait;

    assert!(sqlserver_style::Column::FullName.is_generated());

    // The raw DDL is appended verbatim after the column type.
    let stmt = DbBackend::MySql
        .build(&Schema::new(DbBackend::MySql).create_table_from_entity(sqlserver_style::Entity))
        .to_string();
    assert!(
        stmt.contains("`full_name` varchar(255) NOT NULL AS CONCAT("),
        "unexpected DDL: {stmt}"
    );

    // And it is never written by the ORM.
    let am = sqlserver_style::ActiveModel {
        first_name: Set(Some("John".to_owned())),
        last_name: Set(Some("Doe".to_owned())),
        full_name: Set("stale".to_owned()),
        ..Default::default()
    };
    let sql = sqlserver_style::Entity::insert(am)
        .build(DbBackend::MySql)
        .to_string();
    assert!(!sql.contains("full_name"), "unexpected INSERT: {sql}");
}

mod generated_pk {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "generated_pk")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false, generated = "always")]
        pub id: i32,
        pub value: i32,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[test]
fn generated_metadata_is_exposed_on_column_def() {
    use sea_orm::ColumnTrait;

    let full_name = customer::Column::FullName.def();
    assert!(full_name.is_generated());
    assert!(full_name.generated_on_insert());
    assert!(full_name.generated_on_update());
    assert_eq!(
        full_name.get_generated(),
        Some(sea_orm::GeneratedColumn::Always)
    );

    let on_insert = customer::Column::NameOnInsert.def();
    assert!(on_insert.generated_on_insert());
    assert!(!on_insert.generated_on_update());

    let on_update = customer::Column::NameOnUpdate.def();
    assert!(!on_update.generated_on_insert());
    assert!(on_update.generated_on_update());

    assert!(!customer::Column::FirstName.def().is_generated());
    assert_eq!(
        on_update.get_column_definition(),
        Some("GENERATED ALWAYS AS (first_name) STORED")
    );
    assert_eq!(
        on_insert.get_generated_expression(),
        Some("first_name || ' ' || last_name")
    );
    assert!(on_insert.is_generated_stored());
}

#[test]
fn insert_omits_generated_columns() {
    let am = customer::ActiveModel {
        first_name: Set(Some("John".to_owned())),
        last_name: Set(Some("Doe".to_owned())),
        // These values must never reach the database.
        full_name: Set("stale".to_owned()),
        name_on_insert: Set("stale".to_owned()),
        ..Default::default()
    };

    assert_eq!(
        customer::Entity::insert(am)
            .build(DbBackend::Postgres)
            .to_string(),
        r#"INSERT INTO "customer" ("first_name", "last_name") VALUES ('John', 'Doe')"#
    );
}

#[test]
fn insert_many_omits_generated_columns() {
    let models = [
        customer::Model {
            id: 1,
            first_name: Some("John".to_owned()),
            last_name: Some("Doe".to_owned()),
            full_name: "John Doe".to_owned(),
            name_on_insert: "John Doe".to_owned(),
            name_on_update: "John".to_owned(),
        },
        customer::Model {
            id: 2,
            first_name: Some("Jane".to_owned()),
            last_name: Some("Roe".to_owned()),
            full_name: "Jane Roe".to_owned(),
            name_on_insert: "Jane Roe".to_owned(),
            name_on_update: "Jane".to_owned(),
        },
    ];

    assert_eq!(
        Insert::<customer::ActiveModel>::many(models)
            .build(DbBackend::Postgres)
            .to_string(),
        r#"INSERT INTO "customer" ("id", "first_name", "last_name") VALUES (1, 'John', 'Doe'), (2, 'Jane', 'Roe')"#
    );
}

#[test]
fn update_omits_generated_columns() {
    // Even explicitly `Set`, generated columns must never appear in `SET`.
    let am = customer::ActiveModel {
        id: Set(1),
        first_name: Set(Some("Johnny".to_owned())),
        last_name: Set(Some("Doe".to_owned())),
        full_name: Set("stale".to_owned()),
        name_on_insert: Set("stale".to_owned()),
        name_on_update: Set("stale".to_owned()),
    };

    assert_eq!(
        customer::Entity::update(am)
            .validate()
            .unwrap()
            .build(DbBackend::Postgres)
            .to_string(),
        r#"UPDATE "customer" SET "first_name" = 'Johnny', "last_name" = 'Doe' WHERE "customer"."id" = 1"#
    );
}

#[test]
fn update_many_omits_generated_columns() {
    let am = customer::ActiveModel {
        first_name: Set(Some("Johnny".to_owned())),
        full_name: Set("stale".to_owned()),
        name_on_update: Set("stale".to_owned()),
        ..Default::default()
    };

    assert_eq!(
        customer::Entity::update_many()
            .set(am)
            .build(DbBackend::Postgres)
            .to_string(),
        r#"UPDATE "customer" SET "first_name" = 'Johnny'"#
    );
}

#[test]
fn model_to_active_model_does_not_overwrite_generated() {
    let model = customer::Model {
        id: 7,
        first_name: Some("John".to_owned()),
        last_name: Some("Doe".to_owned()),
        full_name: "John Doe".to_owned(),
        name_on_insert: "John Doe".to_owned(),
        name_on_update: "John".to_owned(),
    };

    let mut am = model.into_active_model();
    // Force every column to `Set`, which is the most aggressive write state.
    am = am.reset_all();

    assert_eq!(
        customer::Entity::update(am)
            .validate()
            .unwrap()
            .build(DbBackend::Postgres)
            .to_string(),
        r#"UPDATE "customer" SET "first_name" = 'John', "last_name" = 'Doe' WHERE "customer"."id" = 7"#
    );
}

#[test]
fn generated_primary_key_is_omitted_from_insert() {
    let am = generated_pk::ActiveModel {
        id: Set(42),
        value: Set(1),
    };

    assert_eq!(
        generated_pk::Entity::insert(am)
            .build(DbBackend::Postgres)
            .to_string(),
        r#"INSERT INTO "generated_pk" ("value") VALUES (1)"#
    );
    // A generated primary key is still used to locate the row on UPDATE.
    let am = generated_pk::ActiveModel {
        id: Set(42),
        value: Set(2),
    };
    assert_eq!(
        generated_pk::Entity::update(am)
            .validate()
            .unwrap()
            .build(DbBackend::Postgres)
            .to_string(),
        r#"UPDATE "generated_pk" SET "value" = 2 WHERE "generated_pk"."id" = 42"#
    );
}

#[test]
fn upsert_insert_path_omits_generated_columns() {
    let am = customer::ActiveModel {
        first_name: Set(Some("John".to_owned())),
        last_name: Set(Some("Doe".to_owned())),
        full_name: Set("stale".to_owned()),
        ..Default::default()
    };

    let sql = customer::Entity::insert(am)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(customer::Column::FirstName)
                .update_column(customer::Column::LastName)
                .to_owned(),
        )
        .build(DbBackend::Postgres)
        .to_string();

    assert!(sql.starts_with(
        r#"INSERT INTO "customer" ("first_name", "last_name") VALUES ('John', 'Doe')"#
    ));
    assert!(!sql.contains("full_name"));
}

#[test]
fn entity_first_create_table_uses_generated_expression() {
    let stmt = DbBackend::Postgres
        .build(&Schema::new(DbBackend::Postgres).create_table_from_entity(customer::Entity))
        .to_string();

    assert!(
        stmt.contains(
            r#""name_on_insert" varchar NOT NULL GENERATED ALWAYS AS (first_name || ' ' || last_name) STORED"#
        ),
        "unexpected DDL: {stmt}"
    );
    assert!(
        stmt.contains(
            r#""name_on_update" varchar NOT NULL GENERATED ALWAYS AS (first_name) STORED"#
        ),
        "unexpected DDL: {stmt}"
    );
    // `generated` alone does not describe DDL, so `full_name` is a plain column.
    assert!(
        stmt.contains(r#""full_name" varchar NOT NULL"#),
        "unexpected DDL: {stmt}"
    );
}

mod virtual_col_entity {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "virtual_col")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub first_name: Option<String>,
        #[sea_orm(
            generated = "always",
            generated_expression = "first_name || ' '",
            generated_stored = false
        )]
        pub display_name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[test]
fn entity_first_create_table_virtual_generated_column() {
    let stmt = DbBackend::MySql
        .build(&Schema::new(DbBackend::MySql).create_table_from_entity(virtual_col_entity::Entity))
        .to_string();
    assert!(
        stmt.contains(
            "`display_name` varchar(255) NOT NULL GENERATED ALWAYS AS (first_name || ' ') VIRTUAL"
        ),
        "unexpected DDL: {stmt}"
    );
}

#[test]
fn column_definition_reaches_ddl_verbatim() {
    let stmt = DbBackend::Postgres
        .build(&Schema::new(DbBackend::Postgres).create_table_from_entity(customer::Entity))
        .to_string();
    assert!(
        stmt.contains("GENERATED ALWAYS AS (first_name) STORED"),
        "unexpected DDL: {stmt}"
    );
}

#[test]
fn column_definition_alone_is_ddl_only() {
    // Not generated: a normal writable column.
    assert!(!raw_definition::Column::Name.is_generated());
    assert!(!raw_definition::Column::Name.generated_on_insert());
    assert!(!raw_definition::Column::Name.generated_on_update());

    let am = raw_definition::ActiveModel {
        name: Set("sea".to_owned()),
        ..Default::default()
    };
    assert_eq!(
        raw_definition::Entity::insert(am)
            .build(DbBackend::Sqlite)
            .to_string(),
        r#"INSERT INTO "raw_definition" ("name") VALUES ('sea')"#
    );

    let stmt = DbBackend::Sqlite
        .build(&Schema::new(DbBackend::Sqlite).create_table_from_entity(raw_definition::Entity))
        .to_string();
    assert!(
        stmt.contains(r#""name" varchar NOT NULL COLLATE NOCASE"#),
        "unexpected DDL: {stmt}"
    );
}

#[test]
fn insert_and_update_refresh_generated_values() -> Result<(), DbErr> {
    smol::block_on(async {
        let db = sea_orm::MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![customer::Model {
                id: 1,
                first_name: Some("John".to_owned()),
                last_name: Some("Doe".to_owned()),
                full_name: "John Doe".to_owned(),
                name_on_insert: "John Doe".to_owned(),
                name_on_update: "John".to_owned(),
            }]])
            .append_query_results([vec![customer::Model {
                id: 1,
                first_name: Some("Johnny".to_owned()),
                last_name: Some("Doe".to_owned()),
                full_name: "Johnny Doe".to_owned(),
                name_on_insert: "Johnny Doe".to_owned(),
                name_on_update: "Johnny".to_owned(),
            }]])
            .into_connection();

        let am = customer::ActiveModel {
            first_name: Set(Some("John".to_owned())),
            last_name: Set(Some("Doe".to_owned())),
            // Stale values must not be sent to the database.
            full_name: Set("stale".to_owned()),
            ..Default::default()
        };
        let inserted = am.insert(&db).await?;
        assert_eq!(inserted.full_name, "John Doe");
        assert_eq!(inserted.name_on_insert, "John Doe");

        let am = customer::ActiveModel {
            id: Set(1),
            first_name: Set(Some("Johnny".to_owned())),
            last_name: Set(Some("Doe".to_owned())),
            full_name: Set("stale".to_owned()),
            name_on_insert: Set("stale".to_owned()),
            name_on_update: Set("stale".to_owned()),
        };
        let updated = am.update(&db).await?;
        assert_eq!(updated.full_name, "Johnny Doe");
        assert_eq!(updated.name_on_update, "Johnny");

        let log = db.into_transaction_log();
        assert_eq!(log.len(), 2);
        assert_eq!(
            log,
            [
                Transaction::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"INSERT INTO "customer" ("first_name", "last_name") VALUES ($1, $2) RETURNING "id", "first_name", "last_name", "full_name", "name_on_insert", "name_on_update""#,
                    ["John".into(), "Doe".into()]
                ),
                Transaction::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"UPDATE "customer" SET "first_name" = $1, "last_name" = $2 WHERE "customer"."id" = $3 RETURNING "id", "first_name", "last_name", "full_name", "name_on_insert", "name_on_update""#,
                    ["Johnny".into(), "Doe".into(), 1i32.into()]
                ),
            ]
        );

        Ok(())
    })
}

#[test]
fn model_to_active_model_update_is_a_noop_and_refreshes() -> Result<(), DbErr> {
    smol::block_on(async {
        let db = sea_orm::MockDatabase::new(DbBackend::Postgres)
            .append_query_results([vec![customer::Model {
                id: 7,
                first_name: Some("John".to_owned()),
                last_name: Some("Doe".to_owned()),
                full_name: "John Doe".to_owned(),
                name_on_insert: "John Doe".to_owned(),
                name_on_update: "John".to_owned(),
            }]])
            .into_connection();

        let model = customer::Model {
            id: 7,
            first_name: Some("John".to_owned()),
            last_name: Some("Doe".to_owned()),
            full_name: "John Doe".to_owned(),
            name_on_insert: "John Doe".to_owned(),
            name_on_update: "John".to_owned(),
        };

        let refreshed = model.into_active_model().update(&db).await?;
        assert_eq!(refreshed.full_name, "John Doe");

        // Because generated (and otherwise unchanged) columns are not written,
        // the UPDATE is a no-op and SeaORM falls back to a SELECT by primary key.
        let log = db.into_transaction_log();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log,
            [Transaction::from_sql_and_values(
                DbBackend::Postgres,
                r#"SELECT "customer"."id", "customer"."first_name", "customer"."last_name", "customer"."full_name", "customer"."name_on_insert", "customer"."name_on_update" FROM "customer" WHERE "customer"."id" = $1 LIMIT $2"#,
                [7i32.into(), 1u64.into()]
            )]
        );

        Ok(())
    })
}

#[test]
fn alter_table_add_column_emits_generated_ddl() {
    use sea_orm::sea_query::TableAlterStatement;

    let schema = Schema::new(DbBackend::Postgres);
    let stmt = DbBackend::Postgres
        .build(
            TableAlterStatement::new()
                .table(customer::Entity)
                .add_column(
                    schema.get_column_def::<customer::Entity>(customer::Column::NameOnInsert),
                ),
        )
        .to_string();
    assert!(
        stmt.contains(
            r#"ADD COLUMN "name_on_insert" varchar NOT NULL GENERATED ALWAYS AS (first_name || ' ' || last_name) STORED"#
        ),
        "unexpected DDL: {stmt}"
    );
}

#[test]
fn update_without_returning_omits_generated_columns() -> Result<(), DbErr> {
    smol::block_on(async {
        let db = sea_orm::MockDatabase::new(DbBackend::Postgres)
            .append_exec_results([sea_orm::MockExecResult {
                last_insert_id: 0,
                rows_affected: 1,
            }])
            .into_connection();

        let am = customer::ActiveModel {
            id: Set(1),
            first_name: Set(Some("Johnny".to_owned())),
            full_name: Set("stale".to_owned()),
            name_on_insert: Set("stale".to_owned()),
            name_on_update: Set("stale".to_owned()),
            ..Default::default()
        };
        let res = am.update_without_returning(&db).await?;
        assert_eq!(res.rows_affected, 1);
        assert_eq!(
            db.into_transaction_log(),
            [Transaction::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE "customer" SET "first_name" = $1 WHERE "customer"."id" = $2"#,
                ["Johnny".into(), 1i32.into()]
            )]
        );

        Ok(())
    })
}

#[test]
fn generated_stored_flag_controls_virtual_ddl() {
    use sea_orm::ColumnTrait;

    let sqlite_stmt = DbBackend::Sqlite
        .build(&Schema::new(DbBackend::Sqlite).create_table_from_entity(virtual_col_entity::Entity))
        .to_string();
    assert!(
        sqlite_stmt.contains("GENERATED ALWAYS AS (first_name || ' ') VIRTUAL"),
        "unexpected DDL: {sqlite_stmt}"
    );
    assert!(virtual_col_entity::Column::DisplayName.def().is_generated());
    assert!(
        !virtual_col_entity::Column::DisplayName
            .def()
            .is_generated_stored()
    );
}

// ---------------------------------------------------------------------------
// Live database integration tests.
//
// Run with, for example:
// DATABASE_URL="sqlite::memory:" cargo test --features sqlx-sqlite,runtime-tokio --test generated_column_tests
// DATABASE_URL="postgres://sea:sea@localhost" cargo test --features sqlx-postgres,runtime-tokio-native-tls --test generated_column_tests
// DATABASE_URL="mysql://sea:sea@localhost" cargo test --features sqlx-mysql,runtime-tokio-native-tls --test generated_column_tests
// ---------------------------------------------------------------------------

mod pg_generated {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "pg_generated")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub first_name: Option<String>,
        pub last_name: Option<String>,
        #[sea_orm(
            generated = "always",
            generated_expression = "COALESCE(first_name, '') || ' ' || COALESCE(last_name, '')"
        )]
        pub full_name: Option<String>,
        #[sea_orm(
            generated = "insert",
            generated_expression = "'ins:' || COALESCE(first_name, '')"
        )]
        pub at_insert: Option<String>,
        #[sea_orm(
            generated = "update",
            generated_expression = "'upd:' || COALESCE(first_name, '')"
        )]
        pub at_update: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

mod my_generated {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "my_generated")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub first_name: Option<String>,
        pub last_name: Option<String>,
        #[sea_orm(
            generated = "always",
            generated_expression = "CONCAT(COALESCE(first_name, ''), ' ', COALESCE(last_name, ''))"
        )]
        pub full_name: Option<String>,
        #[sea_orm(
            generated = "insert",
            generated_expression = "CONCAT('ins:', COALESCE(first_name, ''))"
        )]
        pub at_insert: Option<String>,
        #[sea_orm(
            generated = "update",
            generated_expression = "CONCAT('upd:', COALESCE(first_name, ''))"
        )]
        pub at_update: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Exercise the generated-column lifecycle against a live database.
///
/// `$m` is the entity module (whose generated-expression syntax matches the
/// backend) and `$db` the connection. Keeping this in one macro guarantees the
/// MySQL/MariaDB and PostgreSQL/SQLite paths get identical coverage.
#[allow(unused_macros)]
macro_rules! run_generated_backend_test {
    ($db:expr, $backend:expr, $m:ident) => {{
        use sea_orm::sea_query::{Expr, OnConflict};

        $db.execute(&Schema::new($backend).create_table_from_entity($m::Entity))
            .await
            .unwrap();

        let am = $m::ActiveModel {
            first_name: Set(Some("John".to_owned())),
            last_name: Set(Some("Doe".to_owned())),
            // All stale generated values must be ignored by the ORM.
            full_name: Set(Some("stale".to_owned())),
            at_insert: Set(Some("stale".to_owned())),
            at_update: Set(Some("stale".to_owned())),
            ..Default::default()
        };
        let inserted = am.insert($db).await.unwrap();
        assert_eq!(inserted.full_name, Some("John Doe".to_owned()));
        assert_eq!(inserted.at_insert, Some("ins:John".to_owned()));
        assert_eq!(inserted.at_update, Some("upd:John".to_owned()));

        let am = $m::ActiveModel {
            id: Set(inserted.id),
            first_name: Set(Some("Jane".to_owned())),
            at_insert: Set(Some("stale".to_owned())),
            at_update: Set(Some("stale".to_owned())),
            ..Default::default()
        };
        let updated = am.update($db).await.unwrap();
        assert_eq!(updated.full_name, Some("Jane Doe".to_owned()));
        assert_eq!(updated.at_insert, Some("ins:Jane".to_owned()));
        assert_eq!(updated.at_update, Some("upd:Jane".to_owned()));

        // Bulk insert: generated columns are omitted and computed by the DB.
        let many = [
            $m::ActiveModel {
                first_name: Set(Some("Ann".to_owned())),
                last_name: Set(Some("Lee".to_owned())),
                full_name: Set(Some("stale".to_owned())),
                ..Default::default()
            },
            $m::ActiveModel {
                first_name: Set(Some("Bob".to_owned())),
                last_name: Set(Some("Ray".to_owned())),
                ..Default::default()
            },
        ];
        Insert::<$m::ActiveModel>::many(many)
            .exec($db)
            .await
            .unwrap();
        let all = $m::Entity::find().all($db).await.unwrap();
        assert!(
            all.iter()
                .any(|m| m.full_name.as_deref() == Some("Ann Lee")),
            "bulk insert should compute generated values: {all:?}"
        );

        // update_many recalculates generated values and never writes them.
        $m::Entity::update_many()
            .col_expr($m::Column::FirstName, Expr::value("Zoe"))
            .filter($m::Column::FirstName.eq("Jane"))
            .exec($db)
            .await
            .unwrap();
        let zoe = $m::Entity::find()
            .filter($m::Column::FirstName.eq("Zoe"))
            .one($db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(zoe.full_name.as_deref(), Some("Zoe Doe"));
        assert_eq!(zoe.at_update.as_deref(), Some("upd:Zoe"));

        // Upsert: the insert path omits generated columns and the conflict path
        // updates only the source column; the DB recalculates the rest.
        let upsert = $m::ActiveModel {
            id: Set(inserted.id),
            first_name: Set(Some("Ups".to_owned())),
            full_name: Set(Some("stale".to_owned())),
            at_insert: Set(Some("stale".to_owned())),
            at_update: Set(Some("stale".to_owned())),
            ..Default::default()
        };
        $m::Entity::insert(upsert)
            .on_conflict(
                OnConflict::column($m::Column::Id)
                    .update_column($m::Column::FirstName)
                    .to_owned(),
            )
            .exec($db)
            .await
            .unwrap();
        let upserted = $m::Entity::find_by_id(inserted.id)
            .one($db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(upserted.full_name.as_deref(), Some("Ups Doe"));
        assert_eq!(upserted.at_insert.as_deref(), Some("ins:Ups"));
        assert_eq!(upserted.at_update.as_deref(), Some("upd:Ups"));
    }};
}

mod db_first {
    use sea_orm::entity::prelude::*;

    // Database-first: the table (and its generated column) already exists, so
    // the entity only declares that the database owns `full_name` and does not
    // describe any DDL.
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "db_first_demo")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: i32,
        pub first_name: Option<String>,
        pub last_name: Option<String>,
        #[sea_orm(generated = "always")]
        pub full_name: Option<String>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Map an already-created generated column (database-first workflow).
#[allow(unused_macros)]
macro_rules! run_database_first_test {
    ($db:expr, $ddl:expr, $m:ident) => {{
        $db.execute_unprepared($ddl).await.unwrap();

        let am = $m::ActiveModel {
            id: Set(1),
            first_name: Set(Some("Ada".to_owned())),
            last_name: Set(Some("Lovelace".to_owned())),
            // Must be ignored by the ORM.
            full_name: Set(Some("stale".to_owned())),
            ..Default::default()
        };
        let inserted = am.insert($db).await.unwrap();
        assert_eq!(inserted.full_name.as_deref(), Some("Ada Lovelace"));

        let am = $m::ActiveModel {
            id: Set(1),
            first_name: Set(Some("Grace".to_owned())),
            full_name: Set(Some("stale".to_owned())),
            ..Default::default()
        };
        let updated = am.update($db).await.unwrap();
        assert_eq!(updated.full_name.as_deref(), Some("Grace Lovelace"));

        assert!(ColumnTrait::is_generated(&$m::Column::FullName));
    }};
}

#[sea_orm_macros::test]
async fn main() {
    use sea_orm::{ConnectionTrait, EntityTrait};

    let ctx = common::TestContext::new("generated_column_tests").await;
    let backend = ctx.db.get_database_backend();

    match backend {
        DbBackend::MySql => {
            run_generated_backend_test!(&ctx.db, backend, my_generated);
            run_database_first_test!(
                &ctx.db,
                "CREATE TABLE `db_first_demo` ( \
                    `id` int NOT NULL PRIMARY KEY, \
                    `first_name` varchar(255), \
                    `last_name` varchar(255), \
                    `full_name` varchar(255) GENERATED ALWAYS AS \
                        (CONCAT(COALESCE(`first_name`, ''), ' ', COALESCE(`last_name`, ''))) STORED \
                 )",
                db_first
            );
        }
        DbBackend::Postgres | DbBackend::Sqlite => {
            run_generated_backend_test!(&ctx.db, backend, pg_generated);
            run_database_first_test!(
                &ctx.db,
                "CREATE TABLE db_first_demo ( \
                    id integer NOT NULL PRIMARY KEY, \
                    first_name varchar(255), \
                    last_name varchar(255), \
                    full_name varchar(255) GENERATED ALWAYS AS \
                        (COALESCE(first_name, '') || ' ' || COALESCE(last_name, '')) STORED \
                 )",
                db_first
            );
        }
        _ => unreachable!("unsupported backend: {backend:?}"),
    }

    ctx.delete().await;
}
