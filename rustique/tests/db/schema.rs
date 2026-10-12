use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use rustique::{Db, Error, Executor, MIGRATIONS_TABLE, MigrationStatus, Migrator, Model, Schema};

use crate::databases;

#[rustique::model(table = "orm_s_items")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Item {
    id: i64,
    views: i64,
    active: bool,
    name: String,
    price: Decimal,
    ratio: f64,
    born: Option<NaiveDate>,
    seen_at: Option<NaiveDateTime>,
    data: Option<serde_json::Value>,
    blob: Option<Vec<u8>>,
}

#[tokio::test]
async fn schema_builder_creates_alters_and_drops_tables() {
    for db in databases().await {
        Schema::drop_if_exists(&db, "orm_s_items").await.unwrap();
        Schema::drop_if_exists(&db, "orm_s_renamed").await.unwrap();
        Schema::drop_if_exists(&db, "orm_s_items").await.unwrap(); // missing table is fine

        Schema::create(&db, "orm_s_items", |t| {
            t.id();
            t.big_integer("views").default(0);
            t.boolean("active").default(true);
            t.string("name", 50).unique();
            t.decimal("price", 10, 2).default(Decimal::new(150, 2));
            t.double("ratio").default(0.5);
            t.date("born").nullable();
            t.datetime("seen_at").nullable();
            t.json("data").nullable();
            t.binary("blob").nullable();
            t.string("legacy", 10).nullable().index();
        })
        .await
        .unwrap();
        assert!(Schema::has_table(&db, "orm_s_items").await.unwrap());
        assert!(
            Schema::has_column(&db, "orm_s_items", "legacy")
                .await
                .unwrap()
        );
        assert!(
            !Schema::has_column(&db, "orm_s_items", "nope")
                .await
                .unwrap()
        );

        // Defaults fill the columns the insert leaves out.
        let mut item = Item::create(
            &db,
            Item {
                name: "a".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        // `create` writes every field, so check the defaults with a raw insert.
        db.statement("INSERT INTO orm_s_items (name) VALUES (?)", ["b".into()])
            .await
            .unwrap();
        let b = Item::query()
            .where_(Item::NAME, "=", "b")
            .first(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((b.views, b.active), (0, true));
        assert_eq!((b.price, b.ratio), (Decimal::new(150, 2), 0.5));

        // Every column type round-trips.
        item.born = NaiveDate::from_ymd_opt(2026, 10, 10);
        item.seen_at = item.born.unwrap().and_hms_micro_opt(1, 2, 3, 456_789);
        item.data = Some(serde_json::json!({"a": [1, 2]}));
        item.blob = Some(vec![0, 1, 255]);
        item.save(&db).await.unwrap();
        assert_eq!(Item::find_or_fail(&db, item.id).await.unwrap(), item);

        // The unique index rejects duplicates.
        let duplicate = db
            .statement("INSERT INTO orm_s_items (name) VALUES (?)", ["a".into()])
            .await;
        assert!(
            matches!(
                duplicate,
                Err(Error::Database {
                    code: Some(1062 | 1),
                    ..
                })
            ),
            "{duplicate:?}"
        );

        Schema::table(&db, "orm_s_items", |t| {
            t.string("nick", 20).nullable().unique();
            t.drop_index("orm_s_items_legacy_index");
            t.drop_column("legacy");
            t.rename_column("views", "hits");
        })
        .await
        .unwrap();
        assert!(
            Schema::has_column(&db, "orm_s_items", "nick")
                .await
                .unwrap()
        );
        assert!(
            Schema::has_column(&db, "orm_s_items", "hits")
                .await
                .unwrap()
        );
        assert!(
            !Schema::has_column(&db, "orm_s_items", "legacy")
                .await
                .unwrap()
        );

        Schema::rename(&db, "orm_s_items", "orm_s_renamed")
            .await
            .unwrap();
        assert!(!Schema::has_table(&db, "orm_s_items").await.unwrap());
        Schema::drop(&db, "orm_s_renamed").await.unwrap();
        assert!(!Schema::has_table(&db, "orm_s_renamed").await.unwrap());
        assert!(Schema::drop(&db, "orm_s_renamed").await.is_err());
    }
}

mod m0001_create_orm_m_users {
    use rustique::{Db, Schema};

    pub async fn up(db: &Db) -> rustique::Result<()> {
        Schema::create(db, "orm_m_users", |t| {
            t.id();
            t.string("email", 100).unique();
            t.timestamps();
        })
        .await
    }

    pub async fn down(db: &Db) -> rustique::Result<()> {
        Schema::drop(db, "orm_m_users").await
    }
}

mod m0002_create_orm_m_posts {
    use rustique::{Db, Schema};

    pub async fn up(db: &Db) -> rustique::Result<()> {
        Schema::create(db, "orm_m_posts", |t| {
            t.id();
            t.foreign_id("user_id")
                .constrained("orm_m_users")
                .cascade_on_delete();
            t.string("title", 100);
            t.soft_deletes();
        })
        .await
    }

    pub async fn down(db: &Db) -> rustique::Result<()> {
        Schema::drop(db, "orm_m_posts").await
    }
}

mod m0003_add_name_to_orm_m_users {
    use rustique::{Db, Schema};

    pub async fn up(db: &Db) -> rustique::Result<()> {
        Schema::table(db, "orm_m_users", |t| {
            t.string("name", 50).default("anon");
        })
        .await
    }

    pub async fn down(db: &Db) -> rustique::Result<()> {
        Schema::table(db, "orm_m_users", |t| t.drop_column("name")).await
    }
}

mod m0004_slow {
    use rustique::Db;

    pub async fn up(_: &Db) -> rustique::Result<()> {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        Ok(())
    }

    pub async fn down(_: &Db) -> rustique::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn migrator_table_lock_name_and_timeout() {
    for db in databases().await {
        for table in ["orm_m_custom", "orm_m_custom_lock"] {
            Schema::drop_if_exists(&db, table).await.unwrap();
        }
        let migrator = Migrator::new(rustique::migrations![m0004_slow])
            .table("orm_m_custom")
            .lock_name("orm_m_custom_lock")
            .lock_timeout(std::time::Duration::from_secs(1));
        // One run holds the lock for 3s; the other gives up after 1s.
        let (a, b) = tokio::join!(migrator.migrate(&db), migrator.migrate(&db));
        let ran: Vec<_> = [a, b].into_iter().filter_map(Result::ok).collect();
        assert_eq!(ran, [vec!["m0004_slow"]]);
        assert!(Schema::has_table(&db, "orm_m_custom").await.unwrap());
        // Other tests create and drop the default table meanwhile, so check
        // it did not record this migration; a missing table counts as none.
        let recorded = rustique::QueryBuilder::table(MIGRATIONS_TABLE)
            .where_("migration", "=", "m0004_slow")
            .count(&db)
            .await
            .unwrap_or(0);
        assert_eq!(recorded, 0);
        assert_eq!(migrator.rollback(&db).await.unwrap(), ["m0004_slow"]);
        for table in ["orm_m_custom", "orm_m_custom_lock"] {
            Schema::drop_if_exists(&db, table).await.unwrap();
        }
    }
}

fn status(batches: [Option<i64>; 3]) -> Vec<MigrationStatus> {
    let names = [
        "m0001_create_orm_m_users",
        "m0002_create_orm_m_posts",
        "m0003_add_name_to_orm_m_users",
    ];
    names
        .into_iter()
        .zip(batches)
        .map(|(name, batch)| MigrationStatus { name, batch })
        .collect()
}

async fn reset(db: &Db) {
    for table in [MIGRATIONS_TABLE, "orm_m_posts", "orm_m_users"] {
        Schema::drop_if_exists(db, table).await.unwrap();
    }
}

#[tokio::test]
async fn migrator_runs_and_rolls_back_batches() {
    for db in databases().await {
        reset(&db).await;
        let first = Migrator::new(rustique::migrations![
            m0001_create_orm_m_users,
            m0002_create_orm_m_posts,
        ]);
        let all = Migrator::new(rustique::migrations![
            m0001_create_orm_m_users,
            m0002_create_orm_m_posts,
            m0003_add_name_to_orm_m_users,
        ]);
        assert_eq!(all.status(&db).await.unwrap(), status([None, None, None]));

        // Two deploys migrating at once: the lock lets one run the batch,
        // the other then finds nothing pending.
        let (a, b) = tokio::join!(first.migrate(&db), first.migrate(&db));
        let mut runs = [a.unwrap(), b.unwrap()];
        runs.sort();
        assert_eq!(
            runs,
            [
                vec![],
                vec!["m0001_create_orm_m_users", "m0002_create_orm_m_posts"]
            ]
        );
        assert!(first.migrate(&db).await.unwrap().is_empty());
        assert_eq!(
            all.migrate(&db).await.unwrap(),
            ["m0003_add_name_to_orm_m_users"]
        );
        assert_eq!(
            all.status(&db).await.unwrap(),
            status([Some(1), Some(1), Some(2)])
        );

        // The foreign key cascades and the added column has its default.
        db.statement(
            "INSERT INTO orm_m_users (email) VALUES (?)",
            ["a@x.io".into()],
        )
        .await
        .unwrap();
        let users = db
            .select("SELECT id, name FROM orm_m_users", [])
            .await
            .unwrap();
        assert_eq!(users[0].get::<String>("name").unwrap(), "anon");
        let id: i64 = users[0].get("id").unwrap();
        db.statement(
            "INSERT INTO orm_m_posts (user_id, title) VALUES (?, ?)",
            [id.into(), "hi".into()],
        )
        .await
        .unwrap();
        db.statement("DELETE FROM orm_m_users", []).await.unwrap();
        let posts = db.select("SELECT id FROM orm_m_posts", []).await.unwrap();
        assert!(posts.is_empty());

        // Rollback undoes one batch at a time, newest migration first.
        assert_eq!(
            all.rollback(&db).await.unwrap(),
            ["m0003_add_name_to_orm_m_users"]
        );
        assert!(
            !Schema::has_column(&db, "orm_m_users", "name")
                .await
                .unwrap()
        );
        // A migration recorded but not registered cannot be rolled back.
        let unknown = Migrator::new(Vec::new()).rollback(&db).await;
        assert!(
            matches!(unknown, Err(Error::MigrationNotFound(_))),
            "{unknown:?}"
        );
        assert_eq!(
            all.rollback(&db).await.unwrap(),
            ["m0002_create_orm_m_posts", "m0001_create_orm_m_users"]
        );
        assert!(!Schema::has_table(&db, "orm_m_users").await.unwrap());
        assert_eq!(all.status(&db).await.unwrap(), status([None, None, None]));
        assert!(all.rollback(&db).await.unwrap().is_empty());

        // `--step` undoes migrations across batches; `reset` undoes all.
        first.migrate(&db).await.unwrap();
        all.migrate(&db).await.unwrap();
        assert_eq!(
            all.rollback_steps(&db, 2).await.unwrap(),
            ["m0003_add_name_to_orm_m_users", "m0002_create_orm_m_posts"]
        );
        assert_eq!(
            all.status(&db).await.unwrap(),
            status([Some(1), None, None])
        );
        all.migrate(&db).await.unwrap();
        assert_eq!(
            all.reset(&db).await.unwrap(),
            [
                "m0003_add_name_to_orm_m_users",
                "m0002_create_orm_m_posts",
                "m0001_create_orm_m_users"
            ]
        );
        assert!(!Schema::has_table(&db, "orm_m_users").await.unwrap());

        let seeded = Migrator::new(Vec::new()).seeder(|db| {
            Box::pin(async move {
                db.statement("SELECT 1 FROM orm_m_missing", [])
                    .await
                    .map(drop)
            })
        });
        assert!(seeded.seed(&db).await.is_err(), "the seeder ran");
        assert!(matches!(
            Migrator::new(Vec::new()).seed(&db).await,
            Err(Error::Config(_))
        ));

        reset(&db).await;
    }
}

/// Drops every table of the test schema, so it cannot run beside the other
/// tests: `cargo test --test db -- --ignored fresh_drops_every_table`.
#[tokio::test]
#[ignore = "drops every table of the test database"]
async fn fresh_drops_every_table() {
    for db in databases().await {
        reset(&db).await;
        Schema::create(&db, "orm_m_stray", |t| {
            t.id();
            t.foreign_id("user_id").nullable();
        })
        .await
        .unwrap();
        let all = Migrator::new(rustique::migrations![
            m0001_create_orm_m_users,
            m0002_create_orm_m_posts,
        ]);
        all.migrate(&db).await.unwrap();
        // Posts reference users: the drop must not trip over the foreign key.
        assert_eq!(
            all.fresh(&db).await.unwrap(),
            ["m0001_create_orm_m_users", "m0002_create_orm_m_posts"]
        );
        assert!(!Schema::has_table(&db, "orm_m_stray").await.unwrap());
        assert_eq!(all.status(&db).await.unwrap()[1].batch, Some(1));
        Schema::drop_all_tables(&db).await.unwrap();
        assert!(!Schema::has_table(&db, "orm_m_users").await.unwrap());
    }
}

/// `schema:snapshot` writes the lines `#[rustique::model]` checks with the
/// `schema-check` feature (`rustique-macros/src/schema_check.rs`).
#[tokio::test]
async fn snapshot_lists_columns() {
    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::connect(&url).await.unwrap();
        Schema::drop_if_exists(&db, "orm_s_snapshot").await.unwrap();
        Schema::create(&db, "orm_s_snapshot", |t| {
            t.id();
            t.string("name", 50);
            t.decimal("price", 10, 2).nullable();
            t.datetime("seen_at").nullable();
        })
        .await
        .unwrap();
        let file =
            std::env::temp_dir().join(format!("rustique-schema-{var}-{}.txt", std::process::id()));
        let path = file.display().to_string();
        let args = ["schema:snapshot", "--env", var, "--path", &path].map(str::to_owned);
        rustique::cli::run(&Migrator::new(Vec::new()), args)
            .await
            .unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        std::fs::remove_file(&file).unwrap();
        Schema::drop(&db, "orm_s_snapshot").await.unwrap();

        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.to_lowercase().starts_with("orm_s_snapshot\t"))
            .collect();
        let expected = if var.contains("MYSQL") {
            assert!(text.contains("\ndialect\tmysql\n"), "{text}");
            [
                "orm_s_snapshot\tid\tnot null\tbigint unsigned",
                "orm_s_snapshot\tname\tnot null\tvarchar(50)",
                "orm_s_snapshot\tprice\tnull\tdecimal(10,2)",
                "orm_s_snapshot\tseen_at\tnull\tdatetime(6)",
            ]
        } else {
            assert!(text.contains("\ndialect\toracle\n"), "{text}");
            [
                "ORM_S_SNAPSHOT\tID\tnot null\tNUMBER(19,0)",
                "ORM_S_SNAPSHOT\tNAME\tnot null\tVARCHAR2(50)",
                "ORM_S_SNAPSHOT\tPRICE\tnull\tNUMBER(10,2)",
                "ORM_S_SNAPSHOT\tSEEN_AT\tnull\tTIMESTAMP(6)",
            ]
        };
        assert_eq!(lines, expected, "{var}");
    }
}
