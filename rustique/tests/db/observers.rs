use std::sync::Mutex;

use rustique::{Error, Executor, Model, Observer};

use crate::{create_table, databases};

static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn event(name: String) {
    EVENTS.lock().unwrap().push(name);
}

fn take_events() -> Vec<String> {
    std::mem::take(&mut *EVENTS.lock().unwrap())
}

/// Serves any model.
struct Log;

impl<M: Model> Observer<M> for Log {
    async fn saving(_: &mut M, _: &impl Executor) -> rustique::Result<()> {
        event(format!("{}.saving", M::TABLE));
        Ok(())
    }

    async fn created(_: &mut M, _: &impl Executor) -> rustique::Result<()> {
        event(format!("{}.created", M::TABLE));
        Ok(())
    }

    async fn updated(_: &mut M, _: &impl Executor) -> rustique::Result<()> {
        event(format!("{}.updated", M::TABLE));
        Ok(())
    }

    async fn deleted(_: &mut M, _: &impl Executor) -> rustique::Result<()> {
        event(format!("{}.deleted", M::TABLE));
        Ok(())
    }
}

/// Refuses to delete a gadget named "keep"; can query.
struct Guard;

impl Observer<Gadget> for Guard {
    async fn deleting(gadget: &mut Gadget, db: &impl Executor) -> rustique::Result<()> {
        let total = Gadget::query().count(db).await?;
        event(format!("guard {total}"));
        if gadget.name == "keep" {
            return Err(Error::InvalidQuery("gadget is protected".to_owned()));
        }
        Ok(())
    }
}

#[rustique::model(table = "orm_observer_gadgets", hooks, observers(Log, Guard))]
#[derive(Debug, Clone, Default)]
struct Gadget {
    id: i64,
    name: String,
}

impl rustique::Hooks for Gadget {
    async fn saving(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        event("hook saving".to_owned());
        self.name = self.name.trim().to_owned();
        Ok(())
    }
}

#[tokio::test]
async fn observers_run_after_hooks() {
    for db in databases().await {
        create_table(
            &db,
            Gadget::TABLE,
            &[("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)")],
        )
        .await;
        take_events();

        let gadget = Gadget {
            name: " keep ".into(),
            ..Default::default()
        };
        let mut gadget = Gadget::create(&db, gadget).await.unwrap();
        assert_eq!(gadget.name, "keep");
        assert_eq!(
            take_events(),
            [
                "hook saving",
                "orm_observer_gadgets.saving",
                "orm_observer_gadgets.created"
            ]
        );

        let error = gadget.delete(&db).await.unwrap_err();
        assert!(matches!(error, Error::InvalidQuery(_)));
        assert!(gadget.exists(), "the observer aborted the delete");
        assert_eq!(take_events(), ["guard 1"]);

        gadget.name = "toss".into();
        gadget.save(&db).await.unwrap();
        gadget.delete(&db).await.unwrap();
        assert!(!gadget.exists());
        assert_eq!(
            take_events(),
            [
                "hook saving",
                "orm_observer_gadgets.saving",
                "orm_observer_gadgets.updated",
                "guard 1",
                "orm_observer_gadgets.deleted"
            ]
        );
    }
}
