use rustique::Model;
use serde::{Deserialize, Serialize};

use crate::{create_table, databases};

#[derive(Debug, Clone, Copy, PartialEq, Default, rustique::Cast)]
enum Status {
    #[default]
    Pending,
    #[cast(rename = "banned_perm")]
    Banned,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, rustique::Cast)]
#[cast(int)]
enum Priority {
    #[default]
    Low = 1,
    High = 10,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, rustique::Cast)]
#[cast(json)]
struct Settings {
    theme: String,
    tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, rustique::Cast)]
struct Email(String);

#[rustique::model(table = "orm_cast_users")]
#[derive(Debug, Clone, Default, PartialEq)]
struct CastUser {
    id: i64,
    email: Email,
    status: Status,
    priority: Priority,
    settings: Settings,
    archived_as: Option<Status>,
}

#[tokio::test]
async fn casts_round_trip_through_a_model() {
    for db in databases().await {
        create_table(
            &db,
            "orm_cast_users",
            &[
                ("email", "VARCHAR(50)", "VARCHAR2(50 CHAR)"),
                ("status", "VARCHAR(20)", "VARCHAR2(20 CHAR)"),
                ("priority", "INT", "NUMBER(5)"),
                ("settings", "JSON", "CLOB CHECK (settings IS JSON)"),
                ("archived_as", "VARCHAR(20)", "VARCHAR2(20 CHAR)"),
            ],
        )
        .await;

        let user = CastUser::create(
            &db,
            CastUser {
                email: Email("a@b.c".into()),
                status: Status::Banned,
                priority: Priority::High,
                settings: Settings {
                    theme: "dark".into(),
                    tags: vec!["x".into(), "y".into()],
                },
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(CastUser::find_or_fail(&db, user.id).await.unwrap(), user);

        let found = CastUser::query()
            .where_(CastUser::STATUS, "=", Status::Banned)
            .where_(CastUser::PRIORITY, ">=", Priority::High)
            .get(&db)
            .await
            .unwrap();
        assert_eq!(found, [user]);
    }
}
