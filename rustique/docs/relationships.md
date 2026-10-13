# Relationships

A relation is a `Rel<T>` field with an `#[rustique(...)]` attribute. It is not a column.

```rust
# use rustique::prelude::*;
#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub country_id: i64,
    #[rustique(belongs_to(Country))]
    pub country: Rel<Option<Country>>,
    #[rustique(has_one(Profile))]
    pub profile: Rel<Option<Profile>>,
    #[rustique(has_many(Post))]
    pub posts: Rel<Vec<Post>>,
    #[rustique(belongs_to_many(Role))]
    pub roles: Rel<Vec<Role>>,
}
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
```

`Rel<T>` is either `NotLoaded` or `Loaded(T)`. Rust cannot run a query on field access (that needs `async` and a connection), so loading is always explicit.

| Use | Field type |
|---|---|
| `has_one`, `belongs_to`, `morph_one` | `Rel<Option<T>>`, or `Rel<Option<Box<T>>>` when the types nest in a cycle |
| `has_many`, `belongs_to_many`, `has_many_through`, `morph_many` | `Rel<Vec<T>>` |
| `morph_to` | `Rel<Option<E>>`, `E` an enum deriving `rustique::MorphTo` |
| `count` | `Rel<u64>` |

Related models must implement `Clone`.

## Reading a relation

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64, #[rustique(belongs_to(Country))] pub country: Rel<Option<Country>>, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>, #[rustique(belongs_to_many(Role))] pub roles: Rel<Vec<Role>> }
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run(db: Db, mut user: User) -> rustique::Result<()> {
user.posts.loaded()?;        // &Vec<Post>; Err(Error::RelationNotLoaded) if not loaded
user.posts.get();            // Option<&Vec<Post>>
user.posts.is_loaded();      // bool
# Ok(()) }
```

## Three ways to load

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64, #[rustique(belongs_to(Country))] pub country: Rel<Option<Country>>, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>, #[rustique(belongs_to_many(Role))] pub roles: Rel<Vec<Role>> }
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run(db: Db, mut user: User) -> rustique::Result<()> {
// 1. Eager, on the query (avoids N+1): one extra query per relation level
let users = User::query().with("posts").with("country").get(&db).await?;
let mut users = User::query().with("posts.comments").get(&db).await?;   // nested

// 2. Eager, after the fact
user.load(&db, "posts.comments").await?;          // one model
users.load(&db, "posts").await?;                  // a Collection<User>

// 3. Lazy: the generated method returns a Query<Post>, refine it as you like
let recent = user.posts().order_by_desc("id").limit(5).get(&db).await?;
# Ok(()) }
```

`with("posts.comments")` runs 3 statements: the users, their posts, the posts' comments. On both databases each relation query takes at most 1000 parent keys, so a level with more parents runs one statement per 1000 of them.

### Constraining eager loads

`with_where` is Laravel's `with(['posts' => fn ($q) => ...])`:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64, #[rustique(belongs_to(Country))] pub country: Rel<Option<Country>>, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>, #[rustique(belongs_to_many(Role))] pub roles: Rel<Vec<Role>> }
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run(db: Db) -> rustique::Result<()> {
let users = User::query()
    .with_where("posts", |q| q.where_("published", "=", true).order_by_desc("id"))
    .with_where("posts.comments", |q| q.limit(100))   // constrains comments; posts load as above
    .get(&db)
    .await?;
# Ok(()) }
```

The closure gets the related query as a `QueryBuilder`. Its conditions are grouped, so an `OR` cannot escape the key filter. The query runs once for all parents; a `limit` / `offset` still applies per parent, as in Eloquent: `with_where("posts", |q| q.order_by_desc("id").limit(3))` loads the 3 newest posts of each user. The rows are numbered with `ROW_NUMBER() OVER (PARTITION BY foreign key ORDER BY <the closure's order, or the related key>)` in a subquery ([`QueryBuilder::limit_per`](query-builder.md#ordering-grouping-limits)). Lazy queries (`user.posts().limit(3)`) limit as usual. Qualify columns of `belongs_to_many` / `has_many_through` relations (`roles.name`), which join a second table.

### Selecting columns

`with("relation:col1,col2")` reads only those columns of the related models, like Eloquent's `with('posts:id,title')`. For a nested path the columns belong to its last relation:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub name: String, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub title: String, pub body: String, #[rustique(belongs_to(User))] pub user: Rel<Option<Box<User>>>, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64, pub body: String }
# async fn run(db: Db) -> rustique::Result<()> {
let users = User::query()
    .with("posts:id,title")             // posts.id, posts.title
    .with("posts.comments:id,body")     // the comments' columns; posts load as above
    .get(&db)
    .await?;
let posts = Post::query().with("user:id,name").get(&db).await?;
# Ok(()) }
```

Plain names are qualified with the related table, so pivot and through joins stay unambiguous. Fields whose columns were not read take their `Default` (see [partial selects](models.md#retrieving-models)). The parent key the relation matches on is read separately, so leave it out if you like (Eloquent needs it), but select the keys that nested relations use: above, the comments need `posts.id`. `with_where("posts:id,title", |q| ...)` combines both.

### Catching N+1 queries

A lazy relation query in a loop over query results is one query per model. `prevent_lazy_loading` flags it, like Eloquent's `preventLazyLoading`:

```rust
rustique::prevent_lazy_loading(if cfg!(debug_assertions) {
    rustique::LazyLoading::Deny    // the relation query fails with Error::InvalidQuery
} else {
    rustique::LazyLoading::Allow   // the default; Warn logs to target `rustique::lazy` instead
});
```

Only models loaded together with others (`get`, `paginate`, chunks, cursors, eager loads) are flagged; a model from `find` or `first` can query its relations freely, and `load` / `with` are never flagged.

## Relation types and keys

Default keys follow Laravel. Override any of them in the attribute.

### has_one / has_many

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64,
#[rustique(has_many(Post))]                                        // posts.user_id = users.id
# pub posts: Rel<Vec<Post>>,
#[rustique(has_many(Post, foreign_key = "author_id", local_key = "id"))]
# pub authored: Rel<Vec<Post>>,
# }
```

`foreign_key` defaults to `{parent}_id` (snake case of the parent struct name), `local_key` to the parent's key.

### Constraining a relation

`scope = f` adds conditions to every query of the relation: lazy, eager, `where_has`, counts and aggregates. It is Eloquent's `hasMany(Post::class)->where('published', true)`. `f` is a `fn(Query<R>) -> Query<R>` (a path or a closure that captures nothing), so local scopes work inside, and its conditions are grouped, so an `OR` cannot escape the key filter. It works on every relation type.

```rust
# use rustique::prelude::*;
# use rustique::Query;
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, pub created_at: Option<chrono::NaiveDateTime> }
#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    #[rustique(has_many(Post, scope = published))]
    pub published_posts: Rel<Vec<Post>>,
    #[rustique(has_many(Post, scope = |q| q.where_("posts.published", "=", false)))]
    pub drafts: Rel<Vec<Post>>,
    #[rustique(has_one(Post, latest_of_many))]                       // highest posts.id
    pub latest_post: Rel<Option<Post>>,
    #[rustique(has_one(Post, oldest_of_many = "created_at", scope = published))]
    pub first_published_post: Rel<Option<Post>>,
    #[rustique(has_one(Post, of_many(created_at = "max", id = "min")))]
    pub newest_post: Rel<Option<Post>>,                              // ties: lowest id
}

fn published(q: Query<Post>) -> Query<Post> {
    q.where_("posts.published", "=", true)
}
# async fn run(db: Db, user: User) -> rustique::Result<()> {

let latest = user.latest_post().first(&db).await?;
let users = User::query().with("latest_post").with_count("published_posts").get(&db).await?;
# Ok(()) }
```

Writes through the relation (`attach`, factories) ignore the scope, as in Eloquent.

### One of many

`latest_of_many` / `oldest_of_many` on a `has_one` or `morph_one` keep, of each parent's related rows, the one with the highest / lowest key, like Eloquent's `latestOfMany()` / `oldestOfMany()`. `latest_of_many = "created_at"` ranks by that column instead, ties going to the highest (lowest) key; rows where the column is NULL never win, as with Eloquent's `MAX`. `of_many(published_at = "max", id = "min")` ranks by several columns in order, each `"max"` or `"min"`, like Eloquent's `ofMany(['published_at' => 'max', 'id' => 'min'])`; the key breaks remaining ties (highest wins) unless listed. Eager loading, `where_has` and counts pick the same row; `where_has("latest_post", |q| q.where_(...))` tests the latest post, not any post.

The pick is a `ROW_NUMBER() OVER (PARTITION BY foreign key ORDER BY ...)` subquery over the related table, filtered by the soft-delete filter, global scopes and the relation's `scope` (Eloquent's `ofMany` closure), so `oldest_of_many` with `scope = published` above is the oldest published post. Conditions from `with_where` or a lazy query apply after the pick. The related model needs a single-column key.

### belongs_to

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Article { pub id: i64, pub country_id: i64, pub author_id: i64,
#[rustique(belongs_to(Country))]                                   // articles.country_id = countries.id
# pub country: Rel<Option<Country>>,
#[rustique(belongs_to(User, foreign_key = "author_id", owner_key = "id"))]
# pub author: Rel<Option<User>>,
# }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64 }
```

`foreign_key` defaults to `{field}_id`, `owner_key` to the related key.

### belongs_to_many

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64,
#[rustique(belongs_to_many(Role))]                                 // pivot table role_user
# pub roles: Rel<Vec<Role>>,
#[rustique(belongs_to_many(Role, table = "user_roles", foreign_pivot_key = "user_id", related_pivot_key = "role_id"))]
# pub other_roles: Rel<Vec<Role>>,
# }
```

Default pivot table: both snake-case model names, sorted, joined with `_`. Also `parent_key`, `related_key`.

Manage pivot rows through the generated `{field}_pivot()`:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64, #[rustique(belongs_to(Country))] pub country: Rel<Option<Country>>, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>, #[rustique(belongs_to_many(Role))] pub roles: Rel<Vec<Role>> }
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run(db: Db, mut user: User) -> rustique::Result<()> {
let pivot = user.roles_pivot()?;
pivot.attach(&db, [1, 2]).await?;
pivot.detach(&db, [2]).await?;          // rows removed (u64)
pivot.sync(&db, [1, 3]).await?;         // exactly these keys afterwards
pivot.sync_without_detaching(&db, [4]).await?;   // adds 4, keeps 1 and 3
pivot.toggle(&db, [1, 5]).await?;       // detaches 1, attaches 5
pivot.detach_all(&db).await?;
# Ok(()) }
```

### Pivot columns

List extra pivot columns with `pivot = [...]`, and add `pivot_timestamps` when the pivot table has `created_at` / `updated_at` (other names: `pivot_timestamps = ["added", "modified"]`). Related models loaded through the relation (lazily or eagerly) then carry their pivot row:

```rust
# use rustique::prelude::*;
#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    #[rustique(belongs_to_many(Role, pivot = ["level", "expires_at"], pivot_timestamps))]
    pub roles: Rel<Vec<Role>>,
}
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# async fn run(db: Db, user: User) -> rustique::Result<()> {

let pivot = user.roles_pivot()?;
pivot.attach_with(&db, [1, 2], vec![("level", "admin".into())]).await?;   // timestamps filled
pivot.update_existing(&db, 2, vec![("level", "editor".into())]).await?;  // and updated_at

for role in user.roles().get(&db).await? {
    let level: String = role.pivot().unwrap().get("level")?;   // Model::pivot() -> Option<&Row>
}
# Ok(()) }
```

`pivot()` is `None` for models not loaded through a `belongs_to_many` relation. `sync`, `sync_without_detaching` and `toggle` fill the timestamps of the rows they attach.

### has_many_through

```rust
# use rustique::prelude::*;
#[rustique::model(table = "countries")]
#[derive(Debug, Clone, Default)]
pub struct Country {
    pub id: i64,
    #[rustique(has_many_through(Post, through = User))]   // countries -> users.country_id -> posts.user_id
    pub posts: Rel<Vec<Post>>,
}
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
```

Keys: `first_key` (on the through table), `second_key` (on the related table, default `{through}_id`), `local_key`, `second_local_key`.

`has_one_through` takes the same options on a `Rel<Option<R>>` field and holds the first related model:

```rust
# use rustique::prelude::*;
#[rustique::model(table = "mechanics")]
#[derive(Debug, Clone, Default)]
pub struct Mechanic {
    pub id: i64,
    #[rustique(has_one_through(Owner, through = Car))]   // mechanics -> cars.mechanic_id -> owners.car_id
    pub car_owner: Rel<Option<Owner>>,
}
# #[rustique::model(table = "cars")]
# #[derive(Debug, Clone, Default)]
# pub struct Car { pub id: i64, pub mechanic_id: i64 }
# #[rustique::model(table = "owners")]
# #[derive(Debug, Clone, Default)]
# pub struct Owner { pub id: i64, pub car_id: i64 }
```

### Composite keys

Pass arrays:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "lines", incrementing = false)]
# #[derive(Debug, Clone, Default)]
# pub struct Line { #[rustique(primary_key)] pub order_no: i64, #[rustique(primary_key)] pub line_no: i64,
#[rustique(has_many(LineNote, foreign_key = ["order_no", "line_no"], local_key = ["order_no", "line_no"]))]
# pub notes: Rel<Vec<LineNote>>,
# }
# #[rustique::model(table = "line_notes")]
# #[derive(Debug, Clone, Default)]
# pub struct LineNote { pub id: i64, pub order_no: i64, pub line_no: i64,
#[rustique(belongs_to(Line, foreign_key = ["order_no", "line_no"]))]
# pub line: Rel<Option<Line>>,
# }
```

Eager loading uses `(a, b) IN ((?, ?), ...)`.

## Polymorphic relations

```rust
# use rustique::prelude::*;
#[rustique::model(table = "posts")]
#[derive(Debug, Clone, Default)]
pub struct Post {
    pub id: i64,
    #[rustique(morph_many(Comment, name = "commentable"))]
    pub comments: Rel<Vec<Comment>>,
}

#[rustique::model(table = "videos", morph_alias = "App\\Models\\Video")]   // alias only for Laravel data
#[derive(Debug, Clone, Default)]
pub struct Video {
    pub id: i64,
    #[rustique(morph_one(Comment, name = "commentable"))]
    pub top_comment: Rel<Option<Box<Comment>>>,   // boxed: a comment can hold its video
}

#[derive(Debug, Clone, rustique::MorphTo)]
pub enum Commentable {
    Post(Post),
    Video(Video),
}

#[rustique::model(table = "comments")]
#[derive(Debug, Clone, Default)]
pub struct Comment {
    pub id: i64,
    pub commentable_type: String,
    pub commentable_id: i64,
    #[rustique(morph_to)]                  // or morph_to(name = "commentable")
    pub commentable: Rel<Option<Commentable>>,
}
# async fn run(db: Db, comment: Comment) -> rustique::Result<()> {
let parent: Option<Commentable> = comment.commentable(&db).await?;
let comments = Comment::query().with("commentable").get(&db).await?;
# Ok(()) }
```

- `*_type` holds the parent's `Model::MORPH_ALIAS` (default: table name).
- `with("commentable")` runs one query per type present. Nested paths apply to every type.
- Lazy: `comment.commentable(&db).await?` returns `Option<Commentable>`.
- An unknown type is `Error::UnknownMorphType`. An empty type (`''`) or a `NULL` id points nowhere: the field is `None`, as in Eloquent.
- `where_has` does not take a `morph_to` relation; use `where_has_morph` (below).

Filter on the parent of a `morph_to` relation with `where_has_morph`, Laravel's `whereHasMorph`. The second argument lists morph aliases (empty: every variant of the enum); the closure gets each type's query and alias:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "posts")] #[derive(Debug, Clone, Default)] pub struct Post { pub id: i64 }
# #[rustique::model(table = "videos")] #[derive(Debug, Clone, Default)] pub struct Video { pub id: i64 }
# #[derive(Debug, Clone, rustique::MorphTo)] pub enum Commentable { Post(Post), Video(Video) }
# #[rustique::model(table = "comments")] #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub commentable_type: String, pub commentable_id: i64,
#   #[rustique(morph_to)] pub commentable: Rel<Option<Commentable>> }
# async fn run(db: Db) -> rustique::Result<()> {
let on_posts = Comment::query()
    .where_has_morph("commentable", &["posts"], |q, _alias| q.where_("posts.id", ">", 10))
    .get(&db).await?;
let orphans = Comment::query().where_doesnt_have_morph("commentable", &[], |q, _| q).get(&db).await?;
# Ok(()) }
```

### Polymorphic many-to-many

`morph_to_many` and its inverse `morphed_by_many` are `belongs_to_many` through a pivot table that also holds the type, like Laravel's `taggables(tag_id, taggable_id, taggable_type)`:

```rust
# use rustique::prelude::*;
#[rustique::model(table = "posts")]
#[derive(Debug, Clone, Default)]
pub struct Post {
    pub id: i64,
    #[rustique(morph_to_many(Tag, name = "taggable"))]     // pivot table taggables
    pub tags: Rel<Vec<Tag>>,
}

#[rustique::model(table = "tags")]
#[derive(Debug, Clone, Default)]
pub struct Tag {
    pub id: i64,
    #[rustique(morphed_by_many(Post, name = "taggable"))]
    pub posts: Rel<Vec<Post>>,
}
# async fn run(db: Db, post: Post) -> rustique::Result<()> {

post.tags_pivot()?.attach(&db, [1, 2]).await?;   // taggable_type = Post::MORPH_ALIAS
let tags = post.tags().get(&db).await?;
let tags_with_posts = Tag::query().with("posts").get(&db).await?;
# Ok(()) }
```

Defaults: `table` = the name plus `s`; `morph_to_many`: `foreign_pivot_key` `{name}_id`, `related_pivot_key` `{related}_id`; `morphed_by_many`: `foreign_pivot_key` `{parent}_id`, `related_pivot_key` `{name}_id`; the type column is `{name}_type`. Every `belongs_to_many` option works (`pivot`, `pivot_timestamps`, `parent_key`, `related_key`), and so does the pivot API.

## Querying relation existence

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub country_id: i64, #[rustique(belongs_to(Country))] pub country: Rel<Option<Country>>, #[rustique(has_many(Post))] pub posts: Rel<Vec<Post>>, #[rustique(belongs_to_many(Role))] pub roles: Rel<Vec<Role>> }
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run(db: Db, mut user: User) -> rustique::Result<()> {
// users with at least one published post
User::query().where_has("posts", |q| q.where_("published", "=", true)).get(&db).await?;

// users with any post
User::query().where_has("posts", |q| q).get(&db).await?;

// users without posts
User::query().where_doesnt_have("posts", |q| q).get(&db).await?;
# Ok(()) }
```

The closure gets a `QueryBuilder` on the related table.

On a self-relation (`parent` / `children` on one table) the inner table is aliased `orm_self`, as Eloquent aliases it `laravel_reserved_0`: qualify its columns as `orm_self.name` in the closure; `users.` refers to the outer row.

## Counting and aggregating related models

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "countries")]
# #[derive(Debug, Clone, Default)]
# pub struct Country { pub id: i64 }
# #[rustique::model(table = "profiles")]
# #[derive(Debug, Clone, Default)]
# pub struct Profile { pub id: i64, pub user_id: i64 }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub published: bool, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "roles")]
# #[derive(Debug, Clone, Default)]
# pub struct Role { pub id: i64 }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    #[rustique(has_many(Post))]
    pub posts: Rel<Vec<Post>>,
    #[rustique(count)]
    pub posts_count: Rel<u64>,      // name: {relation}_count
}
# async fn run(db: Db) -> rustique::Result<()> {

let users = User::query().with_count("posts").get(&db).await?;
let n: u64 = *users[0].posts_count.loaded()?;

// only some rows: Laravel's withCount(['posts' => fn ($q) => ...])
let users = User::query().with_count_where("posts", |q| q.where_("published", "=", true)).get(&db).await?;
# Ok(()) }
```

`with_sum`, `with_avg`, `with_min` and `with_max` (Laravel's `withSum`, ...) add `{relation}_{function}_{column}`, and `with_exists` adds `{relation}_exists`. Each has a `_where` variant taking a closure that filters the related rows (`with_sum_where("posts", "votes", |q| ...)`). Read them into `#[rustique(aggregate)]` fields (the same as `#[rustique(count)]`), with `Option` since an aggregate over no rows is `NULL`:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, pub votes: i64 }
#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    #[rustique(has_many(Post))]
    pub posts: Rel<Vec<Post>>,
    #[rustique(aggregate)]
    pub posts_sum_votes: Rel<Option<i64>>,   // a qualified column `posts.votes` is named after `votes`
    #[rustique(aggregate)]
    pub posts_exists: Rel<bool>,
}
# async fn run(db: Db) -> rustique::Result<()> {

let users = User::query().with_sum("posts", "votes").with_exists("posts").get(&db).await?;
# Ok(()) }
```

Every function of this family accepts `"relation as alias"` to read into another field, like Eloquent's `withCount('posts as published_count')`: `with_count_where("posts as published_count", |q| ...)` fills `published_count`.

`with_exists` selects `CASE WHEN EXISTS (...) THEN 1 ELSE 0 END`, since Oracle 19c has no boolean expressions in the select list. Counts and aggregates work on self-relations too.

Counts on the parents a `morph_to` relation loads, per type, are Laravel's `morphWithCount`: `with_morph_count` eager-loads the relation like `with` and fills `#[rustique(count)]` fields of the listed types:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "posts")] #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, #[rustique(has_many(Tag))] pub tags: Rel<Vec<Tag>>, #[rustique(count)] pub tags_count: Rel<u64> }
# #[rustique::model(table = "tags")] #[derive(Debug, Clone, Default)] pub struct Tag { pub id: i64, pub post_id: i64 }
# #[rustique::model(table = "videos")] #[derive(Debug, Clone, Default)] pub struct Video { pub id: i64 }
# #[derive(Debug, Clone, rustique::MorphTo)] pub enum Commentable { Post(Post), Video(Video) }
# #[rustique::model(table = "comments")] #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub commentable_type: String, pub commentable_id: i64,
#   #[rustique(morph_to)] pub commentable: Rel<Option<Commentable>> }
# async fn run(db: Db) -> rustique::Result<()> {
// (morph alias, relation of that type): posts get `tags_count`, videos nothing
let comments = Comment::query().with_morph_count("commentable", &[("posts", "tags")]).get(&db).await?;
# Ok(()) }
```

Counting a `morph_to` relation itself (it holds at most one model) is not supported; Eloquent rejects it too. Use `where_has_morph` to filter on it.

An unknown relation name makes the query fail with `Error::UnknownRelation`.
