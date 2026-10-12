# Factories

Factories build models with default attributes for tests and seeders, like Laravel's model factories. Implement `HasFactory` (in the prelude) and its `definition`:

```rust
use rustique::prelude::*;

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub admin: bool,
}

impl HasFactory for User {
    fn definition(n: u64) -> Self {
        User {
            name: format!("User {n}"),
            email: format!("user{n}@example.com"),
            ..Default::default()
        }
    }
}
```

`n` is different on every call in the process (1, 2, 3, ...), so columns with a unique index get unique values. For realistic names and texts, call a fake-data crate such as `fake` inside `definition`; the ORM does not ship one.

## Making and creating

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub name: String, pub email: String, pub admin: bool }
# impl HasFactory for User {
#     fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } }
# }
# async fn run(db: Db) -> rustique::Result<()> {
let user = User::factory().make_one();                       // unsaved User
let users = User::factory().count(3).make();                 // Collection<User>, unsaved

let user = User::factory().create_one(&db).await?;           // saved User
let users = User::factory().count(3).create(&db).await?;     // Collection<User>, saved
# Ok(()) }
```

- `create` and `create_one` insert each model with `save`, so hooks and observers run and keys and timestamps are filled.
- `create` stops at the first error and keeps the models it saved before. Pass a `Transaction` for all or nothing, or use `db.rollback_after(...)` in tests (see [Transactions](database.md)).
- A factory is reusable: every `make` / `create` call builds new models with new values of `n`.

## States and sequences

`state` changes every model after its definition. `sequence` also gets the model's index in the batch, from 0. They run in the order they were added.

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub name: String, pub email: String, pub admin: bool }
# impl HasFactory for User {
#     fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } }
# }
let admin = User::factory().state(|u| u.admin = true).make_one();
assert!(admin.admin);

let users = User::factory()
    .count(4)
    .sequence(|u, i| u.admin = i % 2 == 0)              // admin, user, admin, user
    .make();
assert_eq!(users.iter().filter(|u| u.admin).count(), 2);
```

Named states, Laravel's `->suspended()`, are plain functions or an extension trait on `Factory<User>`:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub name: String, pub email: String, pub admin: bool }
# impl HasFactory for User {
#     fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } }
# }
use rustique::Factory;

pub trait UserFactory {
    fn admin(self) -> Self;
}

impl UserFactory for Factory<User> {
    fn admin(self) -> Self {
        self.state(|u| u.admin = true)
    }
}

assert!(User::factory().admin().make_one().admin);
```

## Relationships

Relations are named as in the model, like Laravel's magic methods.

### Has: children of each created model

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "teams")]
# #[derive(Debug, Clone, Default)]
# pub struct Team { pub id: i64, pub name: String }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User {
#     pub id: i64, pub team_id: i64, pub email: String,
#     #[rustique(belongs_to(Team))] pub team: Rel<Option<Team>>,
#     #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>,
#     #[rustique(belongs_to_many(Role, pivot = ["level"]))] pub roles: Rel<Vec<Role>>,
#     #[rustique(morph_many(Image, name = "imageable"))] pub images: Rel<Vec<Image>>,
# }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, #[rustique(belongs_to(User))] pub user: Rel<Option<User>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64, pub name: String }
# #[rustique::model(table = "images")]
# #[derive(Debug, Clone, Default)]
# pub struct Image { pub id: i64, pub imageable_type: String, pub imageable_id: i64 }
# impl HasFactory for Team { fn definition(n: u64) -> Self { Team { name: format!("Team {n}"), ..Default::default() } } }
# impl HasFactory for User { fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } } }
# impl HasFactory for Post { fn definition(n: u64) -> Self { Post { title: format!("Post {n}"), ..Default::default() } } }
# impl HasFactory for Role { fn definition(n: u64) -> Self { Role { name: format!("Role {n}"), ..Default::default() } } }
# impl HasFactory for Image { fn definition(_: u64) -> Self { Image::default() } }
# async fn run(db: Db) -> rustique::Result<()> {
let users = User::factory()
    .count(2)
    .has(Post::factory().count(3), "posts")              // has_one / has_many: sets posts.user_id
    .has(Image::factory(), "images")                     // morph_one / morph_many: sets the id and type
    .has_attached(Role::factory().count(2), "roles", vec![("level", "admin".into())])  // belongs_to_many: pivot rows
    .create(&db)
    .await?;
# Ok(()) }
```

`has` on a `belongs_to_many` relation inserts pivot rows without extra columns; `has_attached` sets `pivot` columns on each. Pivot timestamps are filled as by `attach`. Children are created only by `create`, never by `make`. `has_many_through` is not supported: create the intermediate models.

### For: the parent of every model

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "teams")]
# #[derive(Debug, Clone, Default)]
# pub struct Team { pub id: i64, pub name: String }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User {
#     pub id: i64, pub team_id: i64, pub email: String,
#     #[rustique(belongs_to(Team))] pub team: Rel<Option<Team>>,
#     #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>,
#     #[rustique(belongs_to_many(Role, pivot = ["level"]))] pub roles: Rel<Vec<Role>>,
#     #[rustique(morph_many(Image, name = "imageable"))] pub images: Rel<Vec<Image>>,
# }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, #[rustique(belongs_to(User))] pub user: Rel<Option<User>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64, pub name: String }
# #[rustique::model(table = "images")]
# #[derive(Debug, Clone, Default)]
# pub struct Image { pub id: i64, pub imageable_type: String, pub imageable_id: i64 }
# impl HasFactory for Team { fn definition(n: u64) -> Self { Team { name: format!("Team {n}"), ..Default::default() } } }
# impl HasFactory for User { fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } } }
# impl HasFactory for Post { fn definition(n: u64) -> Self { Post { title: format!("Post {n}"), ..Default::default() } } }
# impl HasFactory for Role { fn definition(n: u64) -> Self { Role { name: format!("Role {n}"), ..Default::default() } } }
# impl HasFactory for Image { fn definition(_: u64) -> Self { Image::default() } }
# async fn run(db: Db, team: Team) -> rustique::Result<()> {
let users = User::factory().count(3).for_(&team, "team").create(&db).await?;   // existing parent

let posts = Post::factory()                                                    // a new parent
    .count(3)
    .for_factory(User::factory().for_factory(Team::factory(), "team"), "user")
    .create(&db)
    .await?;
# Ok(()) }
```

- `for_` takes an existing model and works with `make` too. On a `morph_to` relation it sets the id and type columns (the parent's `MORPH_ALIAS`).
- `for_factory` creates **one** parent per `create` call, shared by all its models. `make` creates nothing: it leaves the foreign key as `definition` set it, unless `recycle` provides a parent.
- Parent keys are set before the states, so a `state` can still change them.

### Recycle: reuse existing models

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "teams")]
# #[derive(Debug, Clone, Default)]
# pub struct Team { pub id: i64, pub name: String }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User {
#     pub id: i64, pub team_id: i64, pub email: String,
#     #[rustique(belongs_to(Team))] pub team: Rel<Option<Team>>,
#     #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>,
#     #[rustique(belongs_to_many(Role, pivot = ["level"]))] pub roles: Rel<Vec<Role>>,
#     #[rustique(morph_many(Image, name = "imageable"))] pub images: Rel<Vec<Image>>,
# }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, #[rustique(belongs_to(User))] pub user: Rel<Option<User>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64, pub name: String }
# #[rustique::model(table = "images")]
# #[derive(Debug, Clone, Default)]
# pub struct Image { pub id: i64, pub imageable_type: String, pub imageable_id: i64 }
# impl HasFactory for Team { fn definition(n: u64) -> Self { Team { name: format!("Team {n}"), ..Default::default() } } }
# impl HasFactory for User { fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } } }
# impl HasFactory for Post { fn definition(n: u64) -> Self { Post { title: format!("Post {n}"), ..Default::default() } } }
# impl HasFactory for Role { fn definition(n: u64) -> Self { Role { name: format!("Role {n}"), ..Default::default() } } }
# impl HasFactory for Image { fn definition(_: u64) -> Self { Image::default() } }
# async fn run(db: Db) -> rustique::Result<()> {
let teams = Team::factory().count(3).create(&db).await?;
let posts = Post::factory()
    .count(10)
    .for_factory(User::factory().for_factory(Team::factory(), "team"), "user")
    .recycle(&teams)                                     // users join these teams; no new team
    .create(&db)
    .await?;
# Ok(()) }
```

When `recycle` provides models of a `for_factory` parent's type, each model takes one of them in turn instead of creating a parent. `recycle` carries over to the factories of `has` and `for_factory`, so nested models reuse them too.

### Without a relation

A state closure works for anything the relation helpers do not cover. State closures are `'static`: copy what they need and `move` it in:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "teams")]
# #[derive(Debug, Clone, Default)]
# pub struct Team { pub id: i64, pub name: String }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User {
#     pub id: i64, pub team_id: i64, pub email: String,
#     #[rustique(belongs_to(Team))] pub team: Rel<Option<Team>>,
#     #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>,
#     #[rustique(belongs_to_many(Role, pivot = ["level"]))] pub roles: Rel<Vec<Role>>,
#     #[rustique(morph_many(Image, name = "imageable"))] pub images: Rel<Vec<Image>>,
# }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, #[rustique(belongs_to(User))] pub user: Rel<Option<User>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64, pub name: String }
# #[rustique::model(table = "images")]
# #[derive(Debug, Clone, Default)]
# pub struct Image { pub id: i64, pub imageable_type: String, pub imageable_id: i64 }
# impl HasFactory for Team { fn definition(n: u64) -> Self { Team { name: format!("Team {n}"), ..Default::default() } } }
# impl HasFactory for User { fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } } }
# impl HasFactory for Post { fn definition(n: u64) -> Self { Post { title: format!("Post {n}"), ..Default::default() } } }
# impl HasFactory for Role { fn definition(n: u64) -> Self { Role { name: format!("Role {n}"), ..Default::default() } } }
# impl HasFactory for Image { fn definition(_: u64) -> Self { Image::default() } }
# async fn run(db: Db, user: User) -> rustique::Result<()> {
let user_id = user.id;
let posts = Post::factory().count(3).state(move |p| p.user_id = user_id).create(&db).await?;
# Ok(()) }
```

## Callbacks

`after_making` runs on each model after its states, in `make` and `create`. `after_creating` runs on each saved model once the whole batch (and its `has` children) is saved, and gets the executor `create` was given as an `AnyExecutor` (a `&Db` or `&Transaction`, usable like either):

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "teams")]
# #[derive(Debug, Clone, Default)]
# pub struct Team { pub id: i64, pub name: String }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User {
#     pub id: i64, pub team_id: i64, pub email: String,
#     #[rustique(belongs_to(Team))] pub team: Rel<Option<Team>>,
#     #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>,
#     #[rustique(belongs_to_many(Role, pivot = ["level"]))] pub roles: Rel<Vec<Role>>,
#     #[rustique(morph_many(Image, name = "imageable"))] pub images: Rel<Vec<Image>>,
# }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, #[rustique(belongs_to(User))] pub user: Rel<Option<User>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64, pub name: String }
# #[rustique::model(table = "images")]
# #[derive(Debug, Clone, Default)]
# pub struct Image { pub id: i64, pub imageable_type: String, pub imageable_id: i64 }
# impl HasFactory for Team { fn definition(n: u64) -> Self { Team { name: format!("Team {n}"), ..Default::default() } } }
# impl HasFactory for User { fn definition(n: u64) -> Self { User { email: format!("user{n}@example.com"), ..Default::default() } } }
# impl HasFactory for Post { fn definition(n: u64) -> Self { Post { title: format!("Post {n}"), ..Default::default() } } }
# impl HasFactory for Role { fn definition(n: u64) -> Self { Role { name: format!("Role {n}"), ..Default::default() } } }
# impl HasFactory for Image { fn definition(_: u64) -> Self { Image::default() } }
let factory = User::factory()
    .after_making(|user| user.email.make_ascii_lowercase())
    .after_creating(|user, db| Box::pin(async move {
        Post::factory().for_(&*user, "user").create(db).await?;
        Ok(())
    }));
```
