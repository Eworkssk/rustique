//! `set_timezone` changes process-wide state, so it gets its own test
//! binary. The database part runs on the databases of `RUSTIQUE_TEST_*_URL`,
//! like `tests/db`.

use chrono::{DateTime, Duration, FixedOffset, NaiveDateTime, Utc};
use rustique::prelude::*;

#[rustique::model(table = "orm_tz_posts", timestamps)]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    published_at: Option<DateTime<Utc>>,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
}

fn close(a: NaiveDateTime, b: NaiveDateTime) -> bool {
    (a - b).abs() < Duration::seconds(5)
}

#[tokio::test]
async fn timestamps_follow_the_application_time_zone() {
    let utc_now = Utc::now().naive_utc();
    assert!(close(rustique::now(), utc_now), "UTC by default");

    rustique::set_timezone(FixedOffset::east_opt(2 * 3600).unwrap());
    let two_hours = Duration::hours(2);
    assert!(close(rustique::now(), utc_now + two_hours));

    // A `DateTime<Utc>` is written as wall-clock time in the zone and read back.
    let instant = DateTime::parse_from_rfc3339("2026-03-01T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let stored = Value::from(instant);
    assert_eq!(
        stored,
        Value::DateTime(instant.naive_utc() + two_hours),
        "written in the zone"
    );
    assert_eq!(DateTime::<Utc>::from_value(stored).unwrap(), instant);

    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::connect(&url).await.unwrap();
        Schema::drop_if_exists(&db, "orm_tz_posts").await.unwrap();
        Schema::create(&db, "orm_tz_posts", |t| {
            t.id();
            t.datetime("published_at").nullable();
            t.timestamps();
        })
        .await
        .unwrap();

        let mut post = Post {
            published_at: Some(instant),
            ..Default::default()
        };
        post.save(&db).await.unwrap();
        let created = post.created_at.unwrap();
        assert!(close(created, Utc::now().naive_utc() + two_hours), "{var}");

        let raw: NaiveDateTime = QueryBuilder::table("orm_tz_posts")
            .value(&db, "published_at")
            .await
            .unwrap();
        assert_eq!(raw, instant.naive_utc() + two_hours, "{var}");
        let found = Post::find(&db, post.id).await.unwrap().unwrap();
        assert_eq!(found.published_at, Some(instant), "{var}");
        assert_eq!(found.created_at, Some(created), "{var}");

        Schema::drop(&db, "orm_tz_posts").await.unwrap();
    }
    rustique::set_timezone(Utc);
}
