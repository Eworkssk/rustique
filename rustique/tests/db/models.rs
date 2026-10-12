use chrono::NaiveDateTime;
use rustique::{Db, Dialect, Error, Executor, Model};

use crate::{create_table, databases, drop_table};

#[rustique::model(table = "orm_people", timestamps, soft_deletes)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Person {
    id: i64,
    name: String,
    email: Option<String>,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

#[tokio::test]
async fn model_crud_with_timestamps_and_soft_deletes() {
    for db in databases().await {
        let (datetime, timestamp) = ("DATETIME(6)", "TIMESTAMP(6)");
        create_table(
            &db,
            "orm_people",
            &[
                ("name", "VARCHAR(50) NOT NULL", "VARCHAR2(50 CHAR) NOT NULL"),
                ("email", "VARCHAR(50)", "VARCHAR2(50 CHAR)"),
                ("created_at", datetime, timestamp),
                ("updated_at", datetime, timestamp),
                ("deleted_at", datetime, timestamp),
            ],
        )
        .await;

        let ann = Person::create(
            &db,
            Person {
                name: "Ann".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(ann.exists() && !ann.is_dirty());
        assert!(ann.id > 0 && ann.created_at.is_some());
        assert_eq!(ann.created_at, ann.updated_at);

        let mut bo = Person {
            name: "Bo".into(),
            email: Some("bo@x.io".into()),
            ..Default::default()
        };
        bo.save(&db).await.unwrap();
        assert!(bo.id > ann.id, "ids: {} then {}", ann.id, bo.id);

        // A loaded model equals the saved one, timestamps included.
        let loaded = Person::find_or_fail(&db, bo.id).await.unwrap();
        assert_eq!(loaded, bo);
        assert!(loaded.exists() && !loaded.is_dirty());

        // Only changed columns (plus updated_at) are written.
        bo.name = "Bob".into();
        assert_eq!(bo.dirty(), [("name", "Bob".into())]);
        assert_eq!(bo.get_original("name"), Some(&"Bo".into()));
        std::thread::sleep(std::time::Duration::from_millis(5));
        bo.save(&db).await.unwrap();
        assert!(!bo.is_dirty());
        assert!(bo.updated_at > bo.created_at);
        assert_eq!(Person::find_or_fail(&db, bo.id).await.unwrap(), bo);

        let names: Vec<String> = Person::query()
            .order_by(Person::NAME)
            .get(&db)
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Ann", "Bob"]);

        // Soft delete hides the row from queries but keeps it.
        bo.delete(&db).await.unwrap();
        assert!(bo.trashed());
        assert!(Person::find(&db, bo.id).await.unwrap().is_none());
        assert_eq!(Person::all(&db).await.unwrap().len(), 1);
        let trashed = Person::query()
            .only_trashed()
            .first_or_fail(&db)
            .await
            .unwrap();
        assert_eq!(trashed, bo);
        assert_eq!(Person::query().with_trashed().count(&db).await.unwrap(), 2);

        bo.restore(&db).await.unwrap();
        assert!(!bo.trashed());
        assert_eq!(Person::all(&db).await.unwrap().len(), 2);

        bo.force_delete(&db).await.unwrap();
        assert!(!bo.exists());
        assert_eq!(Person::query().with_trashed().count(&db).await.unwrap(), 1);
        assert!(matches!(
            Person::find_or_fail(&db, bo.id).await,
            Err(Error::NotFound("orm_people"))
        ));

        // Query-level delete is soft too.
        assert_eq!(Person::query().delete(&db).await.unwrap(), 1);
        assert_eq!(Person::query().count(&db).await.unwrap(), 0);
        assert_eq!(Person::query().with_trashed().count(&db).await.unwrap(), 1);
    }
}

#[rustique::model(table = "orm_seq_items")]
#[derive(Debug, Clone, Default)]
struct SeqItem {
    #[rustique(primary_key, sequence = "orm_seq_items_seq")]
    id: i64,
    label: String,
}

#[rustique::model(table = "orm_trigger_items")]
#[derive(Debug, Clone, Default)]
struct TriggerItem {
    id: i64,
    label: String,
}

/// Creates the two tables: Oracle fills the keys from a sequence named in the
/// model and from a trigger; MySQL uses `AUTO_INCREMENT` for both.
async fn create_generated_key_tables(db: &Db) {
    for table in ["orm_seq_items", "orm_trigger_items"] {
        drop_table(db, table).await;
    }
    if db.dialect() == Dialect::MySql {
        for table in ["orm_seq_items", "orm_trigger_items"] {
            let create = format!(
                "CREATE TABLE {table} (id BIGINT AUTO_INCREMENT PRIMARY KEY, label VARCHAR(20))"
            );
            db.statement(&create, []).await.unwrap();
        }
        return;
    }
    for sequence in ["orm_seq_items_seq", "orm_trigger_items_seq"] {
        let drop = format!(
            "BEGIN EXECUTE IMMEDIATE 'DROP SEQUENCE {sequence}'; \
             EXCEPTION WHEN OTHERS THEN IF SQLCODE != -2289 THEN RAISE; END IF; END;"
        );
        db.statement(&drop, []).await.unwrap();
        // Start high so a key confused with a row count or identity shows up.
        let create = format!("CREATE SEQUENCE {sequence} START WITH 1000");
        db.statement(&create, []).await.unwrap();
    }
    for table in ["orm_seq_items", "orm_trigger_items"] {
        let create =
            format!("CREATE TABLE {table} (id NUMBER(19) PRIMARY KEY, label VARCHAR2(20))");
        db.statement(&create, []).await.unwrap();
    }
    db.statement(
        "CREATE OR REPLACE TRIGGER orm_trigger_items_bi BEFORE INSERT ON orm_trigger_items \
         FOR EACH ROW BEGIN :new.id := orm_trigger_items_seq.NEXTVAL; END;",
        [],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn generated_keys_from_sequence_and_trigger() {
    for db in databases().await {
        create_generated_key_tables(&db).await;
        let first = if db.dialect() == Dialect::MySql {
            1
        } else {
            1000
        };

        for (i, label) in ["a", "b"].into_iter().enumerate() {
            let item = SeqItem::create(
                &db,
                SeqItem {
                    label: label.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(item.id, first + i64::try_from(i).unwrap());
            assert_eq!(
                SeqItem::find_or_fail(&db, item.id).await.unwrap().label,
                label
            );

            let item = TriggerItem::create(
                &db,
                TriggerItem {
                    label: label.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert_eq!(item.id, first + i64::try_from(i).unwrap());
            assert_eq!(
                TriggerItem::find_or_fail(&db, item.id).await.unwrap().label,
                label
            );
        }
    }
}

#[rustique::model(table = "orm_memberships")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Membership {
    #[rustique(primary_key)]
    team_id: i64,
    #[rustique(primary_key)]
    code: String,
    role: String,
}

#[tokio::test]
async fn composite_primary_key() {
    for db in databases().await {
        drop_table(&db, "orm_memberships").await;
        let create = if db.dialect() == Dialect::MySql {
            "CREATE TABLE orm_memberships (team_id BIGINT, code VARCHAR(10), role VARCHAR(10), \
             PRIMARY KEY (team_id, code))"
        } else {
            "CREATE TABLE orm_memberships (team_id NUMBER(19), code VARCHAR2(10), role VARCHAR2(10), \
             PRIMARY KEY (team_id, code))"
        };
        db.statement(create, []).await.unwrap();

        for (team_id, code) in [(1, "a"), (1, "b"), (2, "a")] {
            Membership::create(
                &db,
                Membership {
                    team_id,
                    code: code.into(),
                    role: "member".into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }

        let mut one = Membership::find_or_fail(&db, (1, "b".into()))
            .await
            .unwrap();
        assert_eq!((one.team_id, one.code.as_str()), (1, "b"));

        // Changing part of the key updates the row found by the original key.
        one.code = "c".into();
        one.role = "admin".into();
        one.save(&db).await.unwrap();
        assert!(
            Membership::find(&db, (1, "b".into()))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            Membership::find_or_fail(&db, (1, "c".into()))
                .await
                .unwrap(),
            one
        );

        one.delete(&db).await.unwrap();
        assert!(
            Membership::find(&db, (1, "c".into()))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(Membership::query().count(&db).await.unwrap(), 2);
    }
}

/// Compile-time check: model futures can be spawned on a multi-threaded runtime.
#[allow(dead_code)]
fn futures_are_send(db: &Db, mut person: Person) {
    fn assert_send<T: Send>(_: T) {}
    assert_send(Person::find(db, 1));
    assert_send(Person::query().get(db));
    assert_send(person.save(db));
}
