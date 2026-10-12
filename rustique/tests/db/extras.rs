use std::sync::Mutex;

use chrono::NaiveDateTime;
use rustique::{Dialect, Error, Executor, Model, Query, QueryBuilder, Value};

use crate::{create_table, databases, drop_table};

const NAME: (&str, &str, &str) = ("name", "VARCHAR(50) NOT NULL", "VARCHAR2(50 CHAR) NOT NULL");
const EMAIL: (&str, &str, &str) = (
    "email",
    "VARCHAR(50) NOT NULL UNIQUE",
    "VARCHAR2(50 CHAR) NOT NULL UNIQUE",
);

#[tokio::test]
async fn pluck_paginate_chunk_and_upsert_rows() {
    for db in databases().await {
        let table = "orm_extras_rows";
        create_table(&db, table, &[EMAIL, NAME]).await;
        let rows = (1..=7)
            .map(|i| {
                vec![
                    Value::from(format!("u{i}@x.io")),
                    Value::from(format!("U{i}")),
                ]
            })
            .collect();
        let query = QueryBuilder::table(table);
        query.insert(&db, &["email", "name"], rows).await.unwrap();

        let names: Vec<String> = query
            .clone()
            .order_by("id")
            .pluck(&db, "name")
            .await
            .unwrap();
        assert_eq!(names, ["U1", "U2", "U3", "U4", "U5", "U6", "U7"]);
        let aliased: Vec<String> = query
            .clone()
            .where_("id", "=", 1)
            .pluck(&db, "orm_extras_rows.name as n")
            .await
            .unwrap();
        assert_eq!(aliased, ["U1"]);

        let page = query
            .clone()
            .order_by("id")
            .paginate(&db, 3, 3)
            .await
            .unwrap();
        assert_eq!((page.total, page.last_page(), page.items.len()), (7, 3, 1));
        assert!(!page.has_more_pages());
        assert_eq!(page.items[0].get::<String>("name").unwrap(), "U7");

        // The OR is grouped, so paging applies to both branches.
        let mut chunks = query
            .clone()
            .where_("name", "=", "U1")
            .or_where("id", ">", 2)
            .order_by_desc("name")
            .chunk_by_id(2, "id");
        let mut seen = Vec::new();
        while let Some(rows) = chunks.next(&db).await.unwrap() {
            assert!(rows.len() <= 2);
            seen.extend(rows.iter().map(|r| r.get::<i64>("id").unwrap()));
        }
        assert_eq!(seen, [1, 3, 4, 5, 6, 7]);

        // Two rows clash on the unique email, one is new.
        let upsert = vec![
            vec![Value::from("u1@x.io"), Value::from("Ann")],
            vec![Value::from("u2@x.io"), Value::from("Bo")],
            vec![Value::from("u8@x.io"), Value::from("Cy")],
        ];
        query
            .upsert(&db, &["email", "name"], upsert, &["email"], &["name"])
            .await
            .unwrap();
        let names: Vec<String> = query
            .clone()
            .order_by("id")
            .pluck(&db, "name")
            .await
            .unwrap();
        assert_eq!(names, ["Ann", "Bo", "U3", "U4", "U5", "U6", "U7", "Cy"]);

        // Without update columns, clashing rows stay as they are.
        let row = vec![vec![Value::from("u1@x.io"), Value::from("Zed")]];
        query
            .upsert(&db, &["email", "name"], row, &["email"], &[])
            .await
            .unwrap();
        let first: Vec<String> = query
            .clone()
            .where_("id", "=", 1)
            .pluck(&db, "name")
            .await
            .unwrap();
        assert_eq!(first, ["Ann"]);
        assert_eq!(query.count(&db).await.unwrap(), 8);

        let keyed: Vec<(String, i64)> = query
            .clone()
            .where_in("id", [2, 3])
            .order_by("id")
            .pluck_keyed(&db, "id", "orm_extras_rows.email")
            .await
            .unwrap();
        assert_eq!(keyed, [("u2@x.io".into(), 2), ("u3@x.io".into(), 3)]);

        drop_table(&db, table).await;
    }
}

/// Hook calls of [`Member`], in order.
static EVENTS: Mutex<Vec<&str>> = Mutex::new(Vec::new());

fn event(name: &'static str) {
    EVENTS.lock().unwrap().push(name);
}

fn take_events() -> Vec<&'static str> {
    std::mem::take(&mut *EVENTS.lock().unwrap())
}

#[rustique::model(table = "orm_extras_members", timestamps, soft_deletes, hooks)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Member {
    id: i64,
    email: String,
    name: String,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

impl rustique::Hooks for Member {
    async fn saving(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("saving");
        self.email = self.email.to_lowercase();
        Ok(())
    }

    async fn saved(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("saved");
        Ok(())
    }

    async fn creating(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("creating");
        Ok(())
    }

    async fn created(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        assert!(self.id > 0, "the key is set before `created`");
        event("created");
        Ok(())
    }

    async fn updating(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("updating");
        Ok(())
    }

    async fn updated(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("updated");
        Ok(())
    }

    async fn deleting(&mut self, db: &impl Executor) -> rustique::Result<()> {
        event("deleting");
        // Hooks can query; the last member cannot be deleted.
        if Member::query().count(db).await? == 1 {
            return Err(Error::InvalidQuery("cannot delete the last member".into()));
        }
        Ok(())
    }

    async fn deleted(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("deleted");
        Ok(())
    }

    async fn force_deleting(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("force_deleting");
        Ok(())
    }

    async fn force_deleted(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("force_deleted");
        Ok(())
    }

    async fn restoring(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("restoring");
        Ok(())
    }

    async fn restored(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("restored");
        Ok(())
    }
}

#[rustique::scopes]
impl Member {
    fn scope_named(q: Query<Self>, prefix: &str) -> Query<Self> {
        q.where_(Member::NAME, "like", format!("{prefix}%"))
    }
}

#[tokio::test]
async fn model_hooks_bulk_writes_and_scopes() {
    for db in databases().await {
        let (datetime, timestamp) = ("DATETIME(6)", "TIMESTAMP(6)");
        let table = Member::TABLE;
        create_table(
            &db,
            table,
            &[
                EMAIL,
                NAME,
                ("created_at", datetime, timestamp),
                ("updated_at", datetime, timestamp),
                ("deleted_at", datetime, timestamp),
            ],
        )
        .await;
        take_events();

        let mut ann = Member::create(
            &db,
            Member {
                email: "ANN@X.IO".into(),
                name: "Ann".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(ann.email, "ann@x.io");
        assert_eq!(take_events(), ["saving", "creating", "created", "saved"]);

        ann.save(&db).await.unwrap();
        assert_eq!(take_events(), ["saving", "saved"], "nothing to update");
        ann.name = "Anna".into();
        ann.save(&db).await.unwrap();
        assert_eq!(take_events(), ["saving", "updating", "updated", "saved"]);

        let error = ann.delete(&db).await.unwrap_err();
        assert!(error.to_string().contains("last member"), "{error}");
        assert!(ann.exists());
        assert_eq!(take_events(), ["deleting"]);

        // Bulk writes fill timestamps, skip the generated key and run no hooks.
        let new = |email: &str, name: &str| Member {
            email: email.into(),
            name: name.into(),
            ..Default::default()
        };
        let mut batch = [new("bo@x.io", "Bo"), new("cy@x.io", "Cy")];
        Member::insert_many(&db, &mut batch).await.unwrap();
        assert!(batch[0].id > ann.id && batch[1].id > batch[0].id);
        for member in &batch {
            assert!(member.exists() && !member.is_dirty());
            assert_eq!(&Member::find_or_fail(&db, member.id).await.unwrap(), member);
        }

        // All or nothing, outside and inside a transaction.
        let mut clash = [new("ed@x.io", "Ed"), new("cy@x.io", "Cy2")];
        assert!(Member::insert_many(&db, &mut clash).await.is_err());
        assert_eq!(Member::query().count(&db).await.unwrap(), 3);
        let tx = db.begin().await.unwrap();
        let mut ok = [new("fi@x.io", "Fi")];
        Member::insert_many(&tx, &mut ok).await.unwrap();
        assert!(Member::insert_many(&tx, &mut clash).await.is_err());
        tx.commit().await.unwrap();
        let fi = Member::find_or_fail(&db, ok[0].id).await.unwrap();
        assert_eq!(fi.name, "Fi");
        fi.clone().force_delete(&db).await.unwrap();
        take_events();
        Member::upsert(
            &db,
            &[new("bo@x.io", "Bob"), new("di@x.io", "Di")],
            &[Member::EMAIL],
            &[Member::NAME],
        )
        .await
        .unwrap();
        assert!(take_events().is_empty());
        let members = Member::query().order_by(Member::ID).get(&db).await.unwrap();
        let names: Vec<&str> = members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Anna", "Bob", "Cy", "Di"]);
        assert!(
            members
                .iter()
                .all(|m| m.created_at.is_some() && m.updated_at.is_some())
        );

        let page = Member::named("C").paginate(&db, 1, 10).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].name, "Cy");
        assert!(page.items[0].exists());

        let mut chunks = Member::query().chunk_by_id(3);
        let mut sizes = Vec::new();
        while let Some(members) = chunks.next(&db).await.unwrap() {
            sizes.push(members.len());
        }
        assert_eq!(sizes, [3, 1]);

        let mut bob = Member::query()
            .named("Bob")
            .first_or_fail(&db)
            .await
            .unwrap();
        bob.delete(&db).await.unwrap();
        assert!(bob.trashed());
        assert_eq!(take_events(), ["deleting", "deleted"]);

        bob.restore(&db).await.unwrap();
        assert!(!bob.trashed());
        let restore = [
            "restoring",
            "saving",
            "updating",
            "updated",
            "saved",
            "restored",
        ];
        assert_eq!(take_events(), restore);

        bob.force_delete(&db).await.unwrap();
        assert!(!bob.exists());
        let force = ["force_deleting", "deleting", "deleted", "force_deleted"];
        assert_eq!(take_events(), force);
        assert_eq!(Member::query().with_trashed().count(&db).await.unwrap(), 3);

        drop_table(&db, table).await;
    }
}

#[rustique::model(table = "orm_extras_pairs", incrementing = false)]
#[derive(Debug, Clone, Default)]
struct Pair {
    #[rustique(primary_key)]
    a: i64,
    #[rustique(primary_key)]
    b: i64,
}

#[tokio::test]
async fn chunk_by_composite_key() {
    for db in databases().await {
        let int = if db.dialect() == Dialect::MySql {
            "INT"
        } else {
            "NUMBER(10)"
        };
        drop_table(&db, Pair::TABLE).await;
        let create = format!(
            "CREATE TABLE {} (a {int} NOT NULL, b {int} NOT NULL, PRIMARY KEY (a, b))",
            Pair::TABLE
        );
        db.statement(&create, []).await.unwrap();
        let mut pairs: Vec<Pair> = [(1, 1), (1, 2), (1, 3), (2, 1), (3, 1), (3, 2)]
            .into_iter()
            .map(|(a, b)| Pair {
                a,
                b,
                ..Default::default()
            })
            .collect();
        Pair::insert_many(&db, &mut pairs).await.unwrap();
        assert!(pairs.iter().all(Model::exists));

        let mut chunks = Pair::query().chunk_by_id(4);
        let mut seen = Vec::new();
        while let Some(chunk) = chunks.next(&db).await.unwrap() {
            seen.push(chunk.iter().map(|p| (p.a, p.b)).collect::<Vec<_>>());
        }
        assert_eq!(
            seen,
            [vec![(1, 1), (1, 2), (1, 3), (2, 1)], vec![(3, 1), (3, 2)]]
        );

        drop_table(&db, Pair::TABLE).await;
    }
}
