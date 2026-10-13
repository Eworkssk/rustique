//! Runs the shared ORM scenarios with rustique or diesel and writes, per
//! scenario, the per-operation timings (nanoseconds) and memory figures as
//! JSON: `orm-bench <rustique|diesel> <mysql|oracle> <out.json>`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::BTreeMap;
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

type Out = BTreeMap<String, serde_json::Value>;
type AnyResult<T> = Result<T, Box<dyn std::error::Error>>;

const MYSQL_URL: &str = "mysql://orm:secret@127.0.0.1:13306/orm_test";
const ORACLE_URL: &str = "oracle://orm:secret@127.0.0.1:11521/XEPDB1";

/// Counts heap bytes in use and their peak. Sees every Rust allocation, but not
/// memory C libraries take with `malloc` (libmysqlclient under diesel); the RSS
/// figures cover that.
struct Counting;

static HEAP: AtomicUsize = AtomicUsize::new(0);
static HEAP_PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    let now = HEAP.fetch_add(bytes, Relaxed) + bytes;
    HEAP_PEAK.fetch_max(now, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        HEAP.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, layout, new_size) };
        if !new.is_null() {
            HEAP.fetch_sub(layout.size(), Relaxed);
            grow(new_size);
        }
        new
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// A `/proc/self/status` field such as `VmHWM:`, in bytes.
fn proc_status(key: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| {
            l.strip_prefix(key)?
                .trim()
                .strip_suffix(" kB")?
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0)
        * 1024
}

/// Memory window around one scenario's timed loop.
struct Mem {
    heap: usize,
}

impl Mem {
    fn start() -> Self {
        HEAP_PEAK.store(HEAP.load(Relaxed), Relaxed);
        // "5" resets VmHWM (peak RSS) to the current RSS.
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        Mem {
            heap: HEAP.load(Relaxed),
        }
    }

    fn finish(self, ns: Vec<u64>) -> serde_json::Value {
        serde_json::json!({
            "ns": ns,
            "heap_peak": HEAP_PEAK.load(Relaxed) - self.heap,
            "rss_peak": proc_status("VmHWM:"),
        })
    }
}

/// Runs `$body` a few times untimed, then `$n` times timed, one sample per run.
macro_rules! bench {
    ($out:ident, $name:expr, $n:expr, |$i:ident| $body:expr) => {{
        let n: usize = $n;
        // Warm-up indices follow the timed ones, so updates never repeat a value.
        for $i in n..n + (n / 10).clamp(1, 100) {
            black_box($body);
        }
        let mut ns = Vec::with_capacity(n);
        let mem = Mem::start();
        for $i in 0..n {
            let start = Instant::now();
            black_box($body);
            ns.push(start.elapsed().as_nanos() as u64);
        }
        $out.insert($name.to_string(), mem.finish(ns));
    }};
}

fn now() -> NaiveDateTime {
    Utc::now().naive_utc()
}

fn user_id(i: usize) -> i64 {
    (i % 100_000) as i64 + 1
}

fn age(i: usize) -> i32 {
    18 + (i % 60) as i32
}

const KINDS: [&str; 5] = ["click", "view", "purchase", "signup", "logout"];

/// The JSON column `bench_events.payload`, decoded into a typed struct by both ORMs.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rustique::Cast,
    diesel::expression::AsExpression,
    diesel::deserialize::FromSqlRow,
)]
#[cast(json)]
#[diesel(sql_type = diesel::sql_types::Json)]
pub struct Payload {
    #[serde(rename = "type")]
    pub kind: String,
    pub score: i64,
    pub tags: Vec<String>,
    pub meta: Meta,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub browser: String,
    pub os: String,
    pub screen: Screen,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Screen {
    pub w: i64,
    pub h: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub sku: String,
    pub qty: i64,
}

fn payload(i: usize) -> Payload {
    Payload {
        kind: KINDS[i % 5].into(),
        score: i as i64,
        tags: vec!["alpha".into(), "beta".into(), "gamma".into()],
        meta: Meta {
            browser: "firefox".into(),
            os: "linux".into(),
            screen: Screen { w: 1920, h: 1080 },
        },
        items: vec![
            Item {
                sku: "A1".into(),
                qty: 2,
            },
            Item {
                sku: "B2".into(),
                qty: 1,
            },
        ],
    }
}

mod rq {
    use super::*;
    use rustique::prelude::*;

    #[rustique::model(table = "bench_users", timestamps)]
    #[derive(Debug, Clone, Default)]
    pub struct User {
        pub id: i64,
        pub name: String,
        pub email: String,
        pub age: i32,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
        #[rustique(has_many(Post, foreign_key = "user_id"))]
        pub posts: Rel<Vec<Post>>,
    }

    #[rustique::model(table = "bench_posts", timestamps)]
    #[derive(Debug, Clone, Default)]
    pub struct Post {
        pub id: i64,
        pub user_id: i64,
        pub title: String,
        pub body: String,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[rustique::model(table = "bench_events", timestamps)]
    #[derive(Debug, Clone, Default)]
    pub struct Event {
        pub id: i64,
        pub user_id: i64,
        pub payload: Payload,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    async fn users(db: &Db, offset: usize, limit: usize) -> rustique::Result<Collection<User>> {
        User::query()
            .order_by(User::ID)
            .offset(offset as u64)
            .limit(limit as u64)
            .get(db)
            .await
    }

    async fn eager(db: &Db, offset: usize, limit: usize) -> rustique::Result<Collection<User>> {
        User::query()
            .order_by(User::ID)
            .offset(offset as u64)
            .limit(limit as u64)
            .with("posts")
            .get(db)
            .await
    }

    async fn events(db: &Db, limit: usize) -> rustique::Result<Collection<Event>> {
        Event::query()
            .order_by(Event::ID)
            .limit(limit as u64)
            .get(db)
            .await
    }

    async fn bulk_insert(db: &Db, count: usize) -> rustique::Result<u64> {
        let ts = now();
        let rows = (0..count)
            .map(|j| {
                vec![
                    format!("Bulk {j}").into(),
                    format!("bulk{j}@example.com").into(),
                    age(j).into(),
                    ts.into(),
                    ts.into(),
                ]
            })
            .collect::<Vec<Vec<Value>>>();
        let executed = QueryBuilder::table("bench_users")
            .insert(
                db,
                &["name", "email", "age", "created_at", "updated_at"],
                rows,
            )
            .await?;
        Ok(executed.rows_affected)
    }

    pub async fn run(out: &mut Out, url: &str) -> AnyResult<()> {
        let db = Db::connect(url).await?;
        // Differs per process, so saves never find the values an earlier runner wrote.
        let run = Utc::now().timestamp_micros();

        bench!(out, "find_by_pk", 2000, |i| User::find(&db, user_id(i))
            .await?);
        bench!(out, "where_order_limit_20", 1000, |i| User::query()
            .where_(User::AGE, "=", age(i))
            .order_by(User::ID)
            .limit(20)
            .get(&db)
            .await?);
        bench!(out, "fetch_1000_rows", 100, |_i| users(&db, 0, 1000)
            .await?);
        bench!(out, "fetch_10000_rows", 20, |_i| users(&db, 0, 10_000)
            .await?);
        bench!(out, "fetch_100000_rows", 5, |_i| users(&db, 0, 100_000)
            .await?);
        bench!(out, "eager_50_users_500_posts", 200, |i| eager(
            &db,
            i % 20 * 50,
            50
        )
        .await?);
        bench!(out, "eager_1000_users_10000_posts", 10, |_i| eager(
            &db, 0, 1000
        )
        .await?);
        bench!(out, "json_fetch_1000", 100, |_i| events(&db, 1000).await?);
        bench!(out, "json_fetch_10000", 10, |_i| events(&db, 10_000)
            .await?);
        bench!(out, "json_where_limit_20", 1000, |i| Event::query()
            .where_json("payload->type", "=", KINDS[i % 5])
            .order_by(Event::ID)
            .limit(20)
            .get(&db)
            .await?);

        let mut loaded = users(&db, 0, 1000).await?.into_iter().collect::<Vec<_>>();
        bench!(out, "update_single", 1000, |i| {
            let user = &mut loaded[i % 1000];
            user.name = format!("Renamed {i} {run}");
            user.save(&db).await?
        });
        let mut loaded = events(&db, 1000).await?.into_iter().collect::<Vec<_>>();
        bench!(out, "json_update_single", 1000, |i| {
            let event = &mut loaded[i % 1000];
            event.payload.score = -(i as i64);
            event.payload.meta.os = format!("os {i} {run}");
            event.save(&db).await?
        });
        bench!(out, "insert_single", 1000, |i| User::create(
            &db,
            User {
                name: format!("New {i}"),
                email: format!("new{i}@example.com"),
                age: age(i),
                ..Default::default()
            }
        )
        .await?);
        bench!(out, "json_insert_single", 1000, |i| Event::create(
            &db,
            Event {
                user_id: user_id(i),
                payload: payload(i),
                ..Default::default()
            }
        )
        .await?);
        bench!(out, "bulk_insert_1000", 20, |_i| bulk_insert(&db, 1000)
            .await?);
        bench!(out, "bulk_insert_10000", 5, |_i| bulk_insert(&db, 10_000)
            .await?);
        Ok(())
    }
}

mod ds {
    use super::*;
    use diesel::deserialize::{self, FromSql};
    use diesel::dsl::sql;
    use diesel::mysql::{Mysql, MysqlValue};
    use diesel::prelude::*;
    use diesel::serialize::{self, IsNull, Output, ToSql};
    use diesel::sql_types::{Bool, Json, Text};

    diesel::table! {
        bench_users (id) {
            id -> BigInt,
            name -> Varchar,
            email -> Varchar,
            age -> Integer,
            created_at -> Nullable<Datetime>,
            updated_at -> Nullable<Datetime>,
        }
    }

    diesel::table! {
        bench_posts (id) {
            id -> BigInt,
            user_id -> BigInt,
            title -> Varchar,
            body -> Text,
            created_at -> Nullable<Datetime>,
            updated_at -> Nullable<Datetime>,
        }
    }

    diesel::table! {
        bench_events (id) {
            id -> BigInt,
            user_id -> BigInt,
            payload -> Json,
            created_at -> Nullable<Datetime>,
            updated_at -> Nullable<Datetime>,
        }
    }

    diesel::joinable!(bench_posts -> bench_users (user_id));
    diesel::allow_tables_to_appear_in_same_query!(bench_users, bench_posts);

    use bench_events::dsl as e;
    use bench_users::dsl as u;

    impl FromSql<Json, Mysql> for Payload {
        fn from_sql(value: MysqlValue<'_>) -> deserialize::Result<Self> {
            Ok(serde_json::from_slice(value.as_bytes())?)
        }
    }

    impl ToSql<Json, Mysql> for Payload {
        fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Mysql>) -> serialize::Result {
            serde_json::to_writer(out, self)?;
            Ok(IsNull::No)
        }
    }

    #[derive(Debug, Queryable, Selectable, Identifiable)]
    #[diesel(table_name = bench_users, check_for_backend(Mysql))]
    pub struct User {
        pub id: i64,
        pub name: String,
        pub email: String,
        pub age: i32,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Debug, Queryable, Selectable, Identifiable, Associations)]
    #[diesel(table_name = bench_posts, belongs_to(User), check_for_backend(Mysql))]
    pub struct Post {
        pub id: i64,
        pub user_id: i64,
        pub title: String,
        pub body: String,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Debug, Queryable, Selectable, Identifiable)]
    #[diesel(table_name = bench_events, check_for_backend(Mysql))]
    pub struct Event {
        pub id: i64,
        pub user_id: i64,
        pub payload: Payload,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Insertable)]
    #[diesel(table_name = bench_users)]
    pub struct NewUser {
        pub name: String,
        pub email: String,
        pub age: i32,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Insertable)]
    #[diesel(table_name = bench_events)]
    pub struct NewEvent {
        pub user_id: i64,
        pub payload: Payload,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    fn new_user(prefix: &str, i: usize) -> NewUser {
        let ts = Some(now());
        NewUser {
            name: format!("{prefix} {i}"),
            email: format!("{}{i}@example.com", prefix.to_lowercase()),
            age: age(i),
            created_at: ts,
            updated_at: ts,
        }
    }

    fn users(conn: &mut MysqlConnection, offset: usize, limit: usize) -> QueryResult<Vec<User>> {
        u::bench_users
            .order(u::id)
            .offset(offset as i64)
            .limit(limit as i64)
            .select(User::as_select())
            .load(conn)
    }

    fn eager(
        conn: &mut MysqlConnection,
        offset: usize,
        limit: usize,
    ) -> QueryResult<Vec<(User, Vec<Post>)>> {
        let users = users(conn, offset, limit)?;
        let posts = Post::belonging_to(&users)
            .select(Post::as_select())
            .load(conn)?
            .grouped_by(&users);
        Ok(users.into_iter().zip(posts).collect())
    }

    fn events(conn: &mut MysqlConnection, limit: usize) -> QueryResult<Vec<Event>> {
        e::bench_events
            .order(e::id)
            .limit(limit as i64)
            .select(Event::as_select())
            .load(conn)
    }

    fn bulk_insert(conn: &mut MysqlConnection, count: usize) -> QueryResult<usize> {
        let rows = (0..count).map(|j| new_user("Bulk", j)).collect::<Vec<_>>();
        diesel::insert_into(u::bench_users)
            .values(&rows)
            .execute(conn)
    }

    pub fn run(out: &mut Out) -> AnyResult<()> {
        let conn = &mut MysqlConnection::establish(MYSQL_URL)?;
        // Differs per process, so saves never find the values an earlier runner wrote.
        let run = Utc::now().timestamp_micros();

        bench!(out, "find_by_pk", 2000, |i| u::bench_users
            .find(user_id(i))
            .select(User::as_select())
            .first(conn)
            .optional()?);
        bench!(out, "where_order_limit_20", 1000, |i| u::bench_users
            .filter(u::age.eq(age(i)))
            .order(u::id)
            .limit(20)
            .select(User::as_select())
            .load(conn)?);
        bench!(out, "fetch_1000_rows", 100, |_i| users(conn, 0, 1000)?);
        bench!(out, "fetch_10000_rows", 20, |_i| users(conn, 0, 10_000)?);
        bench!(out, "fetch_100000_rows", 5, |_i| users(conn, 0, 100_000)?);
        bench!(out, "eager_50_users_500_posts", 200, |i| eager(
            conn,
            i % 20 * 50,
            50
        )?);
        bench!(out, "eager_1000_users_10000_posts", 10, |_i| eager(
            conn, 0, 1000
        )?);
        bench!(out, "json_fetch_1000", 100, |_i| events(conn, 1000)?);
        bench!(out, "json_fetch_10000", 10, |_i| events(conn, 10_000)?);
        bench!(out, "json_where_limit_20", 1000, |i| e::bench_events
            .filter(
                sql::<Bool>("JSON_UNQUOTE(JSON_EXTRACT(payload, '$.type')) = ")
                    .bind::<Text, _>(KINDS[i % 5])
            )
            .order(e::id)
            .limit(20)
            .select(Event::as_select())
            .load(conn)?);

        bench!(out, "update_single", 1000, |i| diesel::update(
            u::bench_users.find(user_id(i % 1000))
        )
        .set((
            u::name.eq(format!("Renamed {i} {run}")),
            u::updated_at.eq(Some(now()))
        ))
        .execute(conn)?);
        let mut loaded = events(conn, 1000)?;
        bench!(out, "json_update_single", 1000, |i| {
            let event = &mut loaded[i % 1000];
            event.payload.score = -(i as i64);
            event.payload.meta.os = format!("os {i} {run}");
            diesel::update(e::bench_events.find(event.id))
                .set((e::payload.eq(&event.payload), e::updated_at.eq(Some(now()))))
                .execute(conn)?
        });
        // Diesel does not return the generated key on MySQL; the row is written but not read back.
        bench!(out, "insert_single", 1000, |i| diesel::insert_into(
            u::bench_users
        )
        .values(new_user("New", i))
        .execute(conn)?);
        bench!(out, "json_insert_single", 1000, |i| {
            let ts = Some(now());
            diesel::insert_into(e::bench_events)
                .values(NewEvent {
                    user_id: user_id(i),
                    payload: payload(i),
                    created_at: ts,
                    updated_at: ts,
                })
                .execute(conn)?
        });
        bench!(out, "bulk_insert_1000", 20, |_i| bulk_insert(conn, 1000)?);
        bench!(out, "bulk_insert_10000", 5, |_i| bulk_insert(conn, 10_000)?);
        Ok(())
    }
}

/// Diesel on Oracle through the third-party `diesel-oci` backend. It has no
/// JSON type, so the payload is a `CLOB` read as text.
mod dso {
    use super::*;
    use diesel::deserialize::{self, FromSql};
    use diesel::dsl::sql;
    use diesel::prelude::*;
    use diesel::sql_types::{Bool, Text};
    use diesel_oci::OciConnection;
    use diesel_oci::oracle::{Oracle, OracleValue};

    diesel::table! {
        bench_users (id) {
            id -> BigInt,
            name -> Text,
            email -> Text,
            age -> Integer,
            created_at -> Nullable<Timestamp>,
            updated_at -> Nullable<Timestamp>,
        }
    }

    diesel::table! {
        bench_posts (id) {
            id -> BigInt,
            user_id -> BigInt,
            title -> Text,
            body -> Text,
            created_at -> Nullable<Timestamp>,
            updated_at -> Nullable<Timestamp>,
        }
    }

    diesel::table! {
        bench_events (id) {
            id -> BigInt,
            user_id -> BigInt,
            payload -> Text,
            created_at -> Nullable<Timestamp>,
            updated_at -> Nullable<Timestamp>,
        }
    }

    diesel::joinable!(bench_posts -> bench_users (user_id));
    diesel::allow_tables_to_appear_in_same_query!(bench_users, bench_posts);

    use bench_events::dsl as e;
    use bench_users::dsl as u;

    impl FromSql<Text, Oracle> for Payload {
        fn from_sql(value: OracleValue<'_>) -> deserialize::Result<Self> {
            let text = <String as FromSql<Text, Oracle>>::from_sql(value)?;
            Ok(serde_json::from_str(&text)?)
        }
    }

    #[derive(Debug, Queryable, Selectable, Identifiable)]
    #[diesel(table_name = bench_users, check_for_backend(Oracle))]
    pub struct User {
        pub id: i64,
        pub name: String,
        pub email: String,
        pub age: i32,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Debug, Queryable, Selectable, Identifiable, Associations)]
    #[diesel(table_name = bench_posts, belongs_to(User), check_for_backend(Oracle))]
    pub struct Post {
        pub id: i64,
        pub user_id: i64,
        pub title: String,
        pub body: String,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Debug, Queryable, Selectable, Identifiable)]
    #[diesel(table_name = bench_events, check_for_backend(Oracle))]
    pub struct Event {
        pub id: i64,
        pub user_id: i64,
        pub payload: Payload,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Insertable)]
    #[diesel(table_name = bench_users)]
    pub struct NewUser {
        pub name: String,
        pub email: String,
        pub age: i32,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    #[derive(Insertable)]
    #[diesel(table_name = bench_events)]
    pub struct NewEvent {
        pub user_id: i64,
        pub payload: String,
        pub created_at: Option<NaiveDateTime>,
        pub updated_at: Option<NaiveDateTime>,
    }

    fn new_user(prefix: &str, i: usize) -> NewUser {
        let ts = Some(now());
        NewUser {
            name: format!("{prefix} {i}"),
            email: format!("{}{i}@example.com", prefix.to_lowercase()),
            age: age(i),
            created_at: ts,
            updated_at: ts,
        }
    }

    fn users(conn: &mut OciConnection, offset: usize, limit: usize) -> QueryResult<Vec<User>> {
        u::bench_users
            .order(u::id)
            .offset(offset as i64)
            .limit(limit as i64)
            .select(User::as_select())
            .load(conn)
    }

    fn eager(
        conn: &mut OciConnection,
        offset: usize,
        limit: usize,
    ) -> QueryResult<Vec<(User, Vec<Post>)>> {
        let users = users(conn, offset, limit)?;
        let posts = Post::belonging_to(&users)
            .select(Post::as_select())
            .load(conn)?
            .grouped_by(&users);
        Ok(users.into_iter().zip(posts).collect())
    }

    fn events(conn: &mut OciConnection, limit: usize) -> QueryResult<Vec<Event>> {
        e::bench_events
            .order(e::id)
            .limit(limit as i64)
            .select(Event::as_select())
            .load(conn)
    }

    fn bulk_insert(conn: &mut OciConnection, count: usize) -> QueryResult<usize> {
        let rows = (0..count).map(|j| new_user("Bulk", j)).collect::<Vec<_>>();
        diesel::insert_into(u::bench_users)
            .values(&rows)
            .execute(conn)
    }

    pub fn run(out: &mut Out) -> AnyResult<()> {
        let conn = &mut OciConnection::establish(ORACLE_URL)?;
        // Differs per process, so saves never find the values an earlier runner wrote.
        let run = Utc::now().timestamp_micros();

        bench!(out, "find_by_pk", 2000, |i| u::bench_users
            .find(user_id(i))
            .select(User::as_select())
            .first(conn)
            .optional()?);
        bench!(out, "where_order_limit_20", 1000, |i| u::bench_users
            .filter(u::age.eq(age(i)))
            .order(u::id)
            .limit(20)
            .select(User::as_select())
            .load(conn)?);
        bench!(out, "fetch_1000_rows", 100, |_i| users(conn, 0, 1000)?);
        bench!(out, "fetch_10000_rows", 20, |_i| users(conn, 0, 10_000)?);
        bench!(out, "fetch_100000_rows", 5, |_i| users(conn, 0, 100_000)?);
        bench!(out, "eager_50_users_500_posts", 200, |i| eager(
            conn,
            i % 20 * 50,
            50
        )?);
        bench!(out, "eager_1000_users_10000_posts", 10, |_i| eager(
            conn, 0, 1000
        )?);
        bench!(out, "json_fetch_1000", 100, |_i| events(conn, 1000)?);
        bench!(out, "json_fetch_10000", 10, |_i| events(conn, 10_000)?);
        bench!(out, "json_where_limit_20", 1000, |i| e::bench_events
            .filter(sql::<Bool>("JSON_VALUE(payload, '$.type') = ").bind::<Text, _>(KINDS[i % 5]))
            .order(e::id)
            .limit(20)
            .select(Event::as_select())
            .load(conn)?);

        bench!(out, "update_single", 1000, |i| diesel::update(
            u::bench_users.find(user_id(i % 1000))
        )
        .set((
            u::name.eq(format!("Renamed {i} {run}")),
            u::updated_at.eq(Some(now()))
        ))
        .execute(conn)?);
        let mut loaded = events(conn, 1000)?;
        bench!(out, "json_update_single", 1000, |i| {
            let event = &mut loaded[i % 1000];
            event.payload.score = -(i as i64);
            event.payload.meta.os = format!("os {i} {run}");
            diesel::update(e::bench_events.find(event.id))
                .set((
                    e::payload.eq(serde_json::to_string(&event.payload)?),
                    e::updated_at.eq(Some(now())),
                ))
                .execute(conn)?
        });
        // Like diesel on MySQL, the generated key is not read back.
        bench!(out, "insert_single", 1000, |i| diesel::insert_into(
            u::bench_users
        )
        .values(new_user("New", i))
        .execute(conn)?);
        bench!(out, "json_insert_single", 1000, |i| {
            let ts = Some(now());
            let payload = serde_json::to_string(&payload(i))?;
            diesel::insert_into(e::bench_events)
                .values(NewEvent {
                    user_id: user_id(i),
                    payload,
                    created_at: ts,
                    updated_at: ts,
                })
                .execute(conn)?
        });
        bench!(out, "bulk_insert_1000", 20, |_i| bulk_insert(conn, 1000)?);
        bench!(out, "bulk_insert_10000", 5, |_i| bulk_insert(conn, 10_000)?);
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> AnyResult<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, orm, db, path] = args.as_slice() else {
        return Err("usage: orm-bench <rustique|diesel> <mysql|oracle> <out.json>".into());
    };
    let mut out = Out::new();
    match (orm.as_str(), db.as_str()) {
        ("rustique", "mysql") => rq::run(&mut out, MYSQL_URL).await?,
        ("rustique", "oracle") => rq::run(&mut out, ORACLE_URL).await?,
        ("diesel", "mysql") => ds::run(&mut out)?,
        ("diesel", "oracle") => dso::run(&mut out)?,
        _ => return Err(format!("unknown ORM / database {orm} {db}").into()),
    }
    std::fs::write(path, serde_json::to_string(&out)?)?;
    Ok(())
}
