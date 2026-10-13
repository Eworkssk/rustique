//! Query and model helpers, global scopes, eager-load constraints, pivot
//! columns, cursor pagination, nested transactions and column changes.

use chrono::{NaiveDate, NaiveDateTime};
use rustique::{
    Db, Dialect, Error, Executor, GlobalScope, Model, Query, QueryBuilder, Rel, Schema, Value,
};
use serde_json::json;

use crate::{create_table, databases, drop_table};

fn datetime(db: &Db) -> (&'static str, &'static str) {
    let _ = db;
    ("DATETIME(6)", "TIMESTAMP(6)")
}

fn at(day: u32, h: u32, m: u32, s: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, day)
        .unwrap()
        .and_hms_opt(h, m, s)
        .unwrap()
}

async fn names(db: &Db, query: QueryBuilder) -> Vec<String> {
    query
        .get(db)
        .await
        .unwrap()
        .iter()
        .map(|row| row.get("name").unwrap())
        .collect()
}

#[tokio::test]
async fn dates_json_unions_and_increments() {
    for db in databases().await {
        let (mysql_dt, oracle_dt) = datetime(&db);
        create_table(
            &db,
            "orm_h_events",
            &[
                ("name", "VARCHAR(10)", "VARCHAR2(10 CHAR)"),
                ("happened", mysql_dt, oracle_dt),
                ("settings", "JSON", "CLOB CHECK (settings IS JSON)"),
                ("score", "INT", "NUMBER(10)"),
            ],
        )
        .await;
        let events = QueryBuilder::table("orm_h_events");
        let rows = vec![
            vec![
                "a".into(),
                at(10, 23, 59, 59).into(),
                Value::from(json!({"theme": "dark", "tags": ["x", "y"], "beta": true, "level": 5})),
                1.into(),
            ],
            vec![
                "b".into(),
                at(11, 0, 0, 0).into(),
                Value::from(json!({"theme": "light", "level": 10})),
                2.into(),
            ],
            vec![
                "c".into(),
                at(11, 12, 0, 0).into(),
                Value::from(json!({"theme": "dark", "level": 2})),
                3.into(),
            ],
        ];
        events
            .insert(&db, &["name", "happened", "settings", "score"], rows)
            .await
            .unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let by_name = || events.clone().order_by("name");

        assert_eq!(
            names(&db, by_name().where_date("happened", "=", day)).await,
            ["b", "c"]
        );
        assert_eq!(
            names(&db, by_name().where_date("happened", "<", day)).await,
            ["a"]
        );
        assert_eq!(
            names(&db, by_name().where_date("happened", "<>", day)).await,
            ["a"]
        );
        assert_eq!(
            names(&db, by_name().where_date("happened", ">=", day)).await,
            ["b", "c"]
        );

        let json = |path: &str, op: &str, value: Value| by_name().where_json(path, op, value);
        assert_eq!(
            names(&db, json("settings->theme", "=", "dark".into())).await,
            ["a", "c"]
        );
        assert_eq!(
            names(&db, json("settings->level", ">", 4.into())).await,
            ["a", "b"]
        );
        assert_eq!(
            names(&db, json("settings->tags->1", "=", "y".into())).await,
            ["a"]
        );
        assert_eq!(
            names(&db, json("settings->beta", "=", true.into())).await,
            ["a"]
        );

        let union = events
            .clone()
            .select(["name"])
            .where_("score", "=", 3)
            .union(events.clone().select(["name"]).where_("score", "=", 1))
            .order_by("name");
        assert_eq!(names(&db, union.clone()).await, ["a", "c"]);
        assert_eq!(union.count(&db).await.unwrap(), 2);
        let all = events
            .clone()
            .select(["name"])
            .union_all(events.clone().select(["name"]).where_("score", "=", 1));
        assert_eq!(all.count(&db).await.unwrap(), 4);
        assert_eq!(names(&db, all.order_by("name").limit(2)).await, ["a", "a"]);

        let a = events.clone().where_("name", "=", "a");
        assert_eq!(a.increment(&db, "score", 5, vec![]).await.unwrap(), 1);
        a.decrement(&db, "score", 2, vec![("name", "a2".into())])
            .await
            .unwrap();
        let score: i64 = events
            .clone()
            .where_("name", "=", "a2")
            .first(&db)
            .await
            .unwrap()
            .unwrap()
            .get("score")
            .unwrap();
        assert_eq!(score, 4);
        drop_table(&db, "orm_h_events").await;
    }
}

#[rustique::model(
    table = "orm_h_accounts",
    timestamps,
    soft_deletes,
    created_at = "created",
    updated_at = "modified",
    deleted_at = "removed"
)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Account {
    id: i64,
    #[rustique(column = "email_address")]
    email: String,
    balance: i64,
    created: Option<NaiveDateTime>,
    modified: Option<NaiveDateTime>,
    removed: Option<NaiveDateTime>,
}

#[tokio::test]
async fn model_helpers_and_renamed_columns() {
    assert_eq!(Account::EMAIL, "email_address");
    assert_eq!(<Account as Model>::CREATED_AT, "created");
    for db in databases().await {
        let (mysql_dt, oracle_dt) = datetime(&db);
        create_table(
            &db,
            "orm_h_accounts",
            &[
                ("email_address", "VARCHAR(30)", "VARCHAR2(30 CHAR)"),
                ("balance", "BIGINT", "NUMBER(19)"),
                ("created", mysql_dt, oracle_dt),
                ("modified", mysql_dt, oracle_dt),
                ("removed", mysql_dt, oracle_dt),
            ],
        )
        .await;
        let ann = || vec![(Account::EMAIL, Value::from("ann@x.io"))];

        let new = Account::first_or_new(&db, ann(), vec![("balance", 3.into())])
            .await
            .unwrap();
        assert!(!new.exists());
        assert_eq!((new.email.as_str(), new.balance), ("ann@x.io", 3));

        let created = Account::first_or_create(&db, ann(), vec![]).await.unwrap();
        assert!(created.exists() && created.id > 0 && created.created.is_some());
        let again = Account::first_or_create(&db, ann(), vec![("balance", 99.into())])
            .await
            .unwrap();
        assert_eq!((again.id, again.balance), (created.id, 0));

        let mut account = Account::update_or_create(&db, ann(), vec![("balance", 10.into())])
            .await
            .unwrap();
        assert_eq!((account.id, account.balance), (created.id, 10));

        account.increment(&db, "balance", 5).await.unwrap();
        account.decrement(&db, "balance", 3).await.unwrap();
        assert_eq!(account.balance, 12);
        assert!(!account.is_dirty());
        let stored = Account::find(&db, account.id).await.unwrap().unwrap();
        assert_eq!(stored.balance, 12);

        // Someone else changes the row; `fresh` sees it, `refresh` adopts it.
        Account::query()
            .where_key(account.id)
            .update(
                &db,
                vec![("balance", 50.into()), ("modified", at(1, 0, 0, 0).into())],
            )
            .await
            .unwrap();
        assert_eq!(account.fresh(&db).await.unwrap().unwrap().balance, 50);
        account.balance = 7; // discarded by `refresh`
        account.refresh(&db).await.unwrap();
        assert_eq!(
            (account.balance, account.modified),
            (50, Some(at(1, 0, 0, 0)))
        );
        assert!(!account.is_dirty());

        account.touch(&db).await.unwrap();
        assert!(account.modified.unwrap() > at(1, 0, 0, 0));

        let mut copy = account.replicate();
        assert_eq!((copy.id, copy.created, copy.balance), (0, None, 50));
        assert!(!copy.exists());
        copy.email = "bo@x.io".into();
        copy.save(&db).await.unwrap();
        assert_ne!(copy.id, account.id);

        let latest = Account::query().latest().first(&db).await.unwrap().unwrap();
        assert_eq!(latest.id, copy.id);

        copy.delete(&db).await.unwrap();
        assert!(copy.removed.is_some());
        assert_eq!(Account::query().count(&db).await.unwrap(), 1);
        assert!(
            copy.fresh(&db).await.unwrap().is_some(),
            "fresh sees trashed rows"
        );

        let mut gone = account.clone();
        Account::query().force_delete(&db).await.unwrap();
        assert!(matches!(gone.refresh(&db).await, Err(Error::NotFound(_))));
        drop_table(&db, "orm_h_accounts").await;
    }
}

struct TenantOne;

impl<M: Model> GlobalScope<M> for TenantOne {
    fn apply(query: Query<M>) -> Query<M> {
        query.where_(format!("{}.tenant_id", M::TABLE), "=", 1)
    }
}

#[rustique::model(table = "orm_h_owners")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Owner {
    id: i64,
    name: String,
    #[rustique(has_many(Doc))]
    docs: Rel<Vec<Doc>>,
    #[rustique(belongs_to_many(Tag, table = "orm_h_doc_tags", foreign_pivot_key = "owner_id", pivot = ["grade"], pivot_timestamps = ["added_at", "changed_at"]))]
    tags: Rel<Vec<Tag>>,
    #[rustique(count)]
    docs_count: Rel<u64>,
}

#[rustique::model(table = "orm_h_docs", global_scopes(TenantOne))]
#[derive(Debug, Clone, Default, PartialEq)]
struct Doc {
    id: i64,
    owner_id: i64,
    tenant_id: i64,
    title: String,
}

#[rustique::model(table = "orm_h_tags")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Tag {
    id: i64,
    name: String,
}

const TEXT: (&str, &str) = ("VARCHAR(20)", "VARCHAR2(20 CHAR)");
const ID: (&str, &str) = ("BIGINT", "NUMBER(19)");

async fn seed_owners(db: &Db) -> Owner {
    create_table(db, "orm_h_owners", &[("name", TEXT.0, TEXT.1)]).await;
    create_table(
        db,
        "orm_h_docs",
        &[
            ("owner_id", ID.0, ID.1),
            ("tenant_id", ID.0, ID.1),
            ("title", TEXT.0, TEXT.1),
        ],
    )
    .await;
    create_table(db, "orm_h_tags", &[("name", TEXT.0, TEXT.1)]).await;
    let (mysql_dt, oracle_dt) = datetime(db);
    create_table(
        db,
        "orm_h_doc_tags",
        &[
            ("owner_id", ID.0, ID.1),
            ("tag_id", ID.0, ID.1),
            ("grade", TEXT.0, TEXT.1),
            ("added_at", mysql_dt, oracle_dt),
            ("changed_at", mysql_dt, oracle_dt),
        ],
    )
    .await;
    let mut owners = vec![
        Owner {
            name: "ann".into(),
            ..Default::default()
        },
        Owner {
            name: "bo".into(),
            ..Default::default()
        },
    ];
    Owner::insert_many(db, &mut owners).await.unwrap();
    let (ann, bo) = (owners[0].id, owners[1].id);
    let doc = |owner_id, tenant_id, title: &str| Doc {
        owner_id,
        tenant_id,
        title: title.into(),
        ..Default::default()
    };
    let mut docs = vec![
        doc(ann, 1, "a1"),
        doc(ann, 1, "a2"),
        doc(ann, 2, "a-other"),
        doc(bo, 1, "b1"),
    ];
    Doc::insert_many(db, &mut docs).await.unwrap();
    let mut tags = vec![
        Tag {
            name: "red".into(),
            ..Default::default()
        },
        Tag {
            name: "blue".into(),
            ..Default::default()
        },
    ];
    Tag::insert_many(db, &mut tags).await.unwrap();
    owners.swap_remove(0)
}

#[tokio::test]
async fn global_scopes_filter_every_query() {
    for db in databases().await {
        let ann = seed_owners(&db).await;
        assert_eq!(Doc::query().count(&db).await.unwrap(), 3);
        assert_eq!(
            Doc::query()
                .without_global_scope::<TenantOne>()
                .count(&db)
                .await
                .unwrap(),
            4
        );
        assert_eq!(
            Doc::query()
                .without_global_scopes()
                .count(&db)
                .await
                .unwrap(),
            4
        );
        // An `OR` in the query cannot escape the scope.
        let either = Doc::query()
            .where_("title", "=", "a-other")
            .or_where("title", "=", "b1");
        assert_eq!(either.count(&db).await.unwrap(), 1);

        assert_eq!(ann.docs().count(&db).await.unwrap(), 2);
        let owners = Owner::query()
            .with("docs")
            .with_count("docs")
            .order_by("id")
            .get(&db)
            .await
            .unwrap();
        assert_eq!(owners[0].docs.loaded().unwrap().len(), 2);
        assert_eq!(owners[0].docs_count.loaded().unwrap(), &2);
        let with_other = Owner::query()
            .where_has("docs", |q| q.where_("title", "=", "a-other"))
            .count(&db)
            .await
            .unwrap();
        assert_eq!(with_other, 0);

        eager_constraints_and_pivot_columns(&db, &ann).await;

        // Bulk updates through `Query` are scoped.
        let renamed = Doc::query()
            .update(&db, vec![("title", "x".into())])
            .await
            .unwrap();
        assert_eq!(renamed, 3);
        for table in ["orm_h_owners", "orm_h_docs", "orm_h_tags", "orm_h_doc_tags"] {
            drop_table(&db, table).await;
        }
    }
}

async fn eager_constraints_and_pivot_columns(db: &Db, ann: &Owner) {
    let tags = Tag::query().order_by("id").get(db).await.unwrap();
    let (red, blue) = (tags[0].id, tags[1].id);
    let pivot = ann.tags_pivot().unwrap();
    pivot
        .attach_with(db, [red], vec![("grade", "admin".into())])
        .await
        .unwrap();
    pivot
        .attach_with(db, [blue], vec![("grade", "guest".into())])
        .await
        .unwrap();
    assert_eq!(
        pivot
            .update_existing(db, blue, vec![("grade", "editor".into())])
            .await
            .unwrap(),
        1
    );

    let loaded = ann.tags().order_by("orm_h_tags.id").get(db).await.unwrap();
    let levels: Vec<String> = loaded
        .iter()
        .map(|tag| tag.pivot().unwrap().get("grade").unwrap())
        .collect();
    assert_eq!(levels, ["admin", "editor"]);
    let stamp: Option<NaiveDateTime> = loaded[0].pivot().unwrap().get("added_at").unwrap();
    assert!(stamp.is_some());

    let owners = Owner::query()
        .with_where("tags", |q| q.where_("orm_h_doc_tags.grade", "=", "admin"))
        .with_where("docs", |q| {
            q.where_("title", "=", "a2").or_where("title", "=", "b1")
        })
        .with_count_where("docs", |q| q.where_("title", "like", "a%"))
        .order_by("id")
        .get(db)
        .await
        .unwrap();
    let ann_tags = owners[0].tags.loaded().unwrap();
    assert_eq!(ann_tags.len(), 1);
    assert_eq!(
        ann_tags[0].pivot().unwrap().get::<String>("grade").unwrap(),
        "admin"
    );
    let titles: Vec<&str> = owners[0]
        .docs
        .loaded()
        .unwrap()
        .iter()
        .map(|d| d.title.as_str())
        .collect();
    assert_eq!(titles, ["a2"]);
    assert_eq!(owners[1].docs.loaded().unwrap()[0].title, "b1");
    assert_eq!(owners[0].docs_count.loaded().unwrap(), &2);
    assert!(
        Owner::find(db, ann.id)
            .await
            .unwrap()
            .unwrap()
            .pivot()
            .is_none()
    );

    pivot.sync(db, [red]).await.unwrap();
    assert_eq!(ann.tags().count(db).await.unwrap(), 1);
}

#[rustique::model(table = "orm_h_items")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Item {
    id: i64,
    score: i64,
}

#[tokio::test]
async fn cursor_pagination_walks_both_ways() {
    for db in databases().await {
        create_table(&db, "orm_h_items", &[("score", "INT", "NUMBER(10)")]).await;
        let mut items: Vec<Item> = [3, 1, 2, 3, 1, 2, 3]
            .into_iter()
            .map(|score| Item {
                score,
                ..Default::default()
            })
            .collect();
        Item::insert_many(&db, &mut items).await.unwrap();
        let ids =
            |page: &rustique::CursorPage<Item>| page.items.iter().map(|i| i.id).collect::<Vec<_>>();
        let all: Vec<i64> = items.iter().map(|i| i.id).collect();

        // By key: 3 + 3 + 1.
        let first = Item::query().cursor_paginate(&db, 3, None).await.unwrap();
        assert_eq!(ids(&first), all[..3]);
        assert!(first.prev_cursor.is_none());
        let second = Item::query()
            .cursor_paginate(&db, 3, first.next_cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(ids(&second), all[3..6]);
        let third = Item::query()
            .cursor_paginate(&db, 3, second.next_cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(ids(&third), all[6..]);
        assert!(third.next_cursor.is_none());
        let back = Item::query()
            .cursor_paginate(&db, 3, third.prev_cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(ids(&back), all[3..6]);
        let start = Item::query()
            .cursor_paginate(&db, 3, back.prev_cursor.as_deref())
            .await
            .unwrap();
        assert_eq!(ids(&start), all[..3]);
        assert!(start.prev_cursor.is_none() && start.next_cursor.is_some());

        // Score descending with ties, the key appended: rows of the builder too.
        let ranked = QueryBuilder::table("orm_h_items")
            .order_by_desc("score")
            .order_by("id");
        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = ranked
                .cursor_paginate(&db, 2, cursor.as_deref())
                .await
                .unwrap();
            seen.extend(page.items.iter().map(|r| r.get::<i64>("score").unwrap()));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, [3, 3, 3, 2, 2, 1, 1]);
        assert!(
            ranked
                .cursor_paginate(&db, 2, Some("nonsense"))
                .await
                .is_err()
        );
        assert!(
            QueryBuilder::table("orm_h_items")
                .cursor_paginate(&db, 2, None)
                .await
                .is_err()
        );
        drop_table(&db, "orm_h_items").await;
    }
}

#[tokio::test]
async fn nested_transactions_retries_and_rollback_after() {
    for db in databases().await {
        create_table(&db, "orm_h_ledger", &[("amount", "INT", "NUMBER(10)")]).await;
        let ledger = QueryBuilder::table("orm_h_ledger");
        db.transaction(|tx| {
            let ledger = ledger.clone();
            Box::pin(async move {
                ledger.insert(tx, &["amount"], vec![vec![1.into()]]).await?;
                let inner: rustique::Result<()> = tx
                    .transaction(|tx| {
                        let ledger = ledger.clone();
                        Box::pin(async move {
                            ledger.insert(tx, &["amount"], vec![vec![2.into()]]).await?;
                            Err(Error::InvalidQuery("abort the inner part".into()))
                        })
                    })
                    .await;
                assert!(inner.is_err());
                tx.transaction(|tx| {
                    let ledger = ledger.clone();
                    Box::pin(
                        async move { ledger.insert(tx, &["amount"], vec![vec![3.into()]]).await },
                    )
                })
                .await?;
                Ok::<_, Error>(())
            })
        })
        .await
        .unwrap();
        let amounts: Vec<i64> = ledger
            .clone()
            .order_by("amount")
            .pluck(&db, "amount")
            .await
            .unwrap();
        assert_eq!(amounts, [1, 3]);

        // A deadlock is retried; other errors are not.
        let mut calls = 0;
        let deadlock = |dialect| Error::Database {
            dialect,
            code: Some(if dialect == Dialect::MySql { 1213 } else { 60 }),
            message: "deadlock".into(),
        };
        let dialect = db.dialect();
        let result: rustique::Result<()> = db
            .transaction_with_retries(3, |tx| {
                calls += 1;
                let attempt = calls;
                let ledger = ledger.clone();
                Box::pin(async move {
                    ledger
                        .insert(tx, &["amount"], vec![vec![10.into()]])
                        .await?;
                    if attempt < 3 {
                        Err(deadlock(dialect))
                    } else {
                        Ok(())
                    }
                })
            })
            .await;
        result.unwrap();
        assert_eq!(calls, 3);
        assert_eq!(
            ledger
                .clone()
                .where_("amount", "=", 10)
                .count(&db)
                .await
                .unwrap(),
            1
        );
        let mut other = 0;
        let failed: rustique::Result<()> = db
            .transaction_with_retries(3, |_| {
                other += 1;
                Box::pin(async { Err(Error::InvalidQuery("no".into())) })
            })
            .await;
        assert!(failed.is_err());
        assert_eq!(other, 1);

        let seen = db
            .rollback_after(|tx| {
                let ledger = ledger.clone();
                Box::pin(async move {
                    ledger.delete(tx).await?;
                    ledger.count(tx).await
                })
            })
            .await
            .unwrap();
        assert_eq!(seen, 0);
        assert_eq!(ledger.count(&db).await.unwrap(), 3);
        drop_table(&db, "orm_h_ledger").await;
    }
}

#[tokio::test]
async fn columns_change_in_place() {
    for db in databases().await {
        Schema::drop_if_exists(&db, "orm_h_people").await.unwrap();
        Schema::create(&db, "orm_h_people", |t| {
            t.id();
            t.string("name", 10);
            t.integer("age").nullable();
        })
        .await
        .unwrap();
        let people = QueryBuilder::table("orm_h_people");
        people
            .insert(&db, &["name", "age"], vec![vec!["Ann".into(), 30.into()]])
            .await
            .unwrap();

        Schema::table(&db, "orm_h_people", |t| {
            t.string("name", 100).nullable().change();
            t.integer("age").default(18).change(); // NOT NULL now
        })
        .await
        .unwrap();
        // Running the same change again is a no-op, also on Oracle.
        Schema::table(&db, "orm_h_people", |t| {
            t.integer("age").default(18).change();
        })
        .await
        .unwrap();

        let long = "x".repeat(60);
        people
            .insert(&db, &["name"], vec![vec![long.clone().into()]])
            .await
            .unwrap();
        let age: i64 = people
            .clone()
            .where_("name", "=", long)
            .first(&db)
            .await
            .unwrap()
            .unwrap()
            .get("age")
            .unwrap();
        assert_eq!(age, 18);
        assert!(
            people
                .insert(&db, &["name", "age"], vec![vec!["Bo".into(), Value::Null]])
                .await
                .is_err()
        );
        people
            .insert(&db, &["age"], vec![vec![5.into()]])
            .await
            .unwrap(); // name is nullable now
        Schema::drop(&db, "orm_h_people").await.unwrap();
    }
}

#[rustique::model(table = "orm_h_bulk")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Bulk {
    id: i64,
    n: i64,
}

#[tokio::test]
async fn insert_many_spans_several_blocks() {
    for db in databases().await {
        create_table(&db, "orm_h_bulk", &[("n", "INT", "NUMBER(10)")]).await;
        let mut rows: Vec<Bulk> = (0..250)
            .map(|n| Bulk {
                n,
                ..Default::default()
            })
            .collect();
        Bulk::insert_many(&db, &mut rows).await.unwrap();
        for row in &rows {
            let stored = Bulk::find(&db, row.id).await.unwrap().unwrap();
            assert_eq!(stored.n, row.n);
        }
        let tx = db.begin().await.unwrap();
        let mut more: Vec<Bulk> = (0..120)
            .map(|n| Bulk {
                n,
                ..Default::default()
            })
            .collect();
        Bulk::insert_many(&tx, &mut more).await.unwrap();
        tx.rollback().await.unwrap();
        assert_eq!(Bulk::query().count(&db).await.unwrap(), 250);
        drop_table(&db, "orm_h_bulk").await;
    }
}

#[rustique::model(table = "orm_h_members")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Member {
    id: i64,
    email: String,
    visits: i64,
}

#[tokio::test]
async fn create_or_first_returns_the_row_that_won() {
    for db in databases().await {
        create_table(
            &db,
            "orm_h_members",
            &[
                ("email", "VARCHAR(30) UNIQUE", "VARCHAR2(30 CHAR) UNIQUE"),
                ("visits", "INT", "NUMBER(10)"),
            ],
        )
        .await;
        let ann = || vec![("email", Value::from("ann@x.io"))];
        let first = Member::create_or_first(&db, ann(), vec![("visits", 1.into())])
            .await
            .unwrap();
        // The insert hits the unique index, as a losing racer's would.
        let second = Member::create_or_first(&db, ann(), vec![("visits", 9.into())])
            .await
            .unwrap();
        assert_eq!((second.id, second.visits), (first.id, 1));
        assert!(second.exists());

        // Inside a transaction the failed insert leaves the transaction usable.
        let tx = db.begin().await.unwrap();
        let found = Member::create_or_first(&tx, ann(), vec![]).await.unwrap();
        assert_eq!(found.id, first.id);
        Member::create_or_first(&tx, vec![("email", "bo@x.io".into())], vec![])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(Member::query().count(&db).await.unwrap(), 2);

        // Errors other than the matched row's are passed on.
        let mut clash = Member {
            email: "ann@x.io".into(),
            ..Default::default()
        };
        assert!(clash.save(&db).await.unwrap_err().is_unique_violation());
        drop_table(&db, "orm_h_members").await;
    }
}

#[rustique::model(table = "orm_h_counters")]
#[derive(Debug, Clone, Default)]
struct Counter {
    id: i64,
    views: u32,
    total: rust_decimal::Decimal,
}

#[tokio::test]
async fn increment_leaves_unsigned_and_decimal_fields_clean() {
    for db in databases().await {
        create_table(
            &db,
            "orm_h_counters",
            &[
                ("views", "INT UNSIGNED", "NUMBER(10)"),
                ("total", "DECIMAL(10, 2)", "NUMBER(10, 2)"),
            ],
        )
        .await;
        let mut counter = Counter::create(&db, Counter::default()).await.unwrap();
        counter.increment(&db, "views", 1).await.unwrap();
        counter.increment(&db, "total", 2).await.unwrap();
        assert_eq!(counter.views, 1);
        assert_eq!(counter.total, rust_decimal::Decimal::from(2));
        // A dirty field would make the next `save` overwrite concurrent increments.
        assert!(!counter.is_dirty(), "{:?}", counter.dirty());
        drop_table(&db, "orm_h_counters").await;
    }
}
