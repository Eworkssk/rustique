use chrono::NaiveDateTime;
use rustique::{AnyExecutor, Db, Dialect, Executor, HasFactory, Model, Rel};

use crate::{create_table, databases, drop_table};

#[rustique::model(table = "orm_f_teams", timestamps)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Team {
    id: i64,
    name: String,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
}

#[rustique::model(table = "orm_f_users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    team_id: Option<i64>,
    email: String,
    #[rustique(belongs_to(Team))]
    team: Rel<Option<Team>>,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
    #[rustique(belongs_to_many(Role, table = "orm_f_role_user", pivot = ["grade"]))]
    roles: Rel<Vec<Role>>,
    #[rustique(morph_many(Image, name = "imageable"))]
    images: Rel<Vec<Image>>,
}

#[rustique::model(table = "orm_f_posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    user_id: i64,
    title: String,
    #[rustique(belongs_to(User))]
    user: Rel<Option<User>>,
}

#[rustique::model(table = "orm_f_roles")]
#[derive(Debug, Clone, Default)]
struct Role {
    id: i64,
    name: String,
}

#[rustique::model(table = "orm_f_images")]
#[derive(Debug, Clone, Default)]
struct Image {
    id: i64,
    url: String,
    imageable_type: Option<String>,
    imageable_id: Option<i64>,
}

impl HasFactory for Team {
    fn definition(n: u64) -> Self {
        Team {
            name: format!("Team {n}"),
            ..Default::default()
        }
    }
}

impl HasFactory for User {
    fn definition(n: u64) -> Self {
        User {
            email: format!("user{n}@x.io"),
            ..Default::default()
        }
    }
}

impl HasFactory for Post {
    fn definition(n: u64) -> Self {
        Post {
            title: format!("Post {n}"),
            ..Default::default()
        }
    }
}

impl HasFactory for Role {
    fn definition(n: u64) -> Self {
        Role {
            name: format!("Role {n}"),
            ..Default::default()
        }
    }
}

impl HasFactory for Image {
    fn definition(n: u64) -> Self {
        Image {
            url: format!("/img/{n}.png"),
            ..Default::default()
        }
    }
}

/// Logs every factory hook and callback, to check their order.
#[rustique::model(table = "orm_f_notes")]
#[derive(Debug, Clone, Default)]
struct Note {
    id: i64,
    log: String,
}

impl HasFactory for Note {
    fn definition(_: u64) -> Self {
        Note::default()
    }

    fn after_making(&mut self) {
        self.log.push_str("hook-make ");
    }

    async fn after_creating(&mut self, db: &AnyExecutor<'_>) -> rustique::Result<()> {
        let id = self.id;
        Post::factory()
            .state(move |p| p.user_id = id)
            .create(db)
            .await?;
        self.log.push_str("hook-create ");
        self.save(db).await
    }
}

const ID: (&str, &str) = ("BIGINT", "NUMBER(19)");
const TEXT: (&str, &str) = ("VARCHAR(50)", "VARCHAR2(50 CHAR)");

async fn create_tables(db: &Db) {
    let (datetime, timestamp) = ("DATETIME(6)", "TIMESTAMP(6)");
    create_table(
        db,
        Team::TABLE,
        &[
            ("name", TEXT.0, TEXT.1),
            ("created_at", datetime, timestamp),
            ("updated_at", datetime, timestamp),
        ],
    )
    .await;
    create_table(
        db,
        User::TABLE,
        &[
            ("team_id", ID.0, ID.1),
            ("email", "VARCHAR(50) UNIQUE", "VARCHAR2(50 CHAR) UNIQUE"),
        ],
    )
    .await;
    create_table(
        db,
        Post::TABLE,
        &[("user_id", ID.0, ID.1), ("title", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(db, Role::TABLE, &[("name", TEXT.0, TEXT.1)]).await;
    create_table(
        db,
        Note::TABLE,
        &[("log", "VARCHAR(100)", "VARCHAR2(100 CHAR)")],
    )
    .await;
    create_table(
        db,
        Image::TABLE,
        &[
            ("url", TEXT.0, TEXT.1),
            ("imageable_type", TEXT.0, TEXT.1),
            ("imageable_id", ID.0, ID.1),
        ],
    )
    .await;
    drop_table(db, "orm_f_role_user").await;
    let (id, text) = if db.dialect() == Dialect::MySql {
        (ID.0, TEXT.0)
    } else {
        (ID.1, TEXT.1)
    };
    let pivot = format!(
        "CREATE TABLE orm_f_role_user (user_id {id}, role_id {id}, grade {text}, PRIMARY KEY (user_id, role_id))"
    );
    db.statement(&pivot, []).await.unwrap();
}

#[tokio::test]
async fn factories_create_models_with_relations() {
    for db in databases().await {
        create_tables(&db).await;

        // create / create_one save through `save`: keys and timestamps are set.
        let teams = Team::factory()
            .count(2)
            .sequence(|t, i| t.name = format!("T{i}"))
            .create(&db)
            .await
            .unwrap();
        assert!(
            teams
                .iter()
                .all(|t| t.exists() && t.id > 0 && t.created_at.is_some())
        );
        assert_eq!(teams[1].name, "T1");
        let team = Team::factory().create_one(&db).await.unwrap();
        assert_eq!(Team::find_or_fail(&db, team.id).await.unwrap(), team);

        // has: has_many, belongs_to_many with pivot values, morph_many.
        let users = User::factory()
            .count(2)
            .for_(&team, "team")
            .has(Post::factory().count(3), "posts")
            .has_attached(
                Role::factory().count(2),
                "roles",
                vec![("grade", "admin".into())],
            )
            .has(Image::factory(), "images")
            .create(&db)
            .await
            .unwrap();
        for user in &users {
            assert_eq!(user.team_id, Some(team.id));
            assert_eq!(user.posts().count(&db).await.unwrap(), 3);
            let roles = user.roles().get(&db).await.unwrap();
            assert_eq!(roles.len(), 2);
            let grade: String = roles[0].pivot().unwrap().get("grade").unwrap();
            assert_eq!(grade, "admin");
            let images = user.images().get(&db).await.unwrap();
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].imageable_type.as_deref(), Some(User::TABLE));
        }

        // for_factory: one parent per create call.
        let before = Team::query().count(&db).await.unwrap();
        let users = User::factory()
            .count(3)
            .for_factory(Team::factory().state(|t| t.name = "new".into()), "team")
            .create(&db)
            .await
            .unwrap();
        assert!(users.iter().all(|u| u.team_id == users[0].team_id));
        let new_team = Team::find_or_fail(&db, users[0].team_id.unwrap())
            .await
            .unwrap();
        assert_eq!(new_team.name, "new");
        assert_eq!(Team::query().count(&db).await.unwrap(), before + 1);

        // recycle: no new team, and it reaches the factories of `has`.
        let users = User::factory()
            .count(4)
            .for_factory(Team::factory(), "team")
            .recycle(&teams)
            .create(&db)
            .await
            .unwrap();
        assert_eq!(Team::query().count(&db).await.unwrap(), before + 1);
        let ids: std::collections::HashSet<_> = users.iter().map(|u| u.team_id).collect();
        assert_eq!(ids.len(), 2, "recycled teams are used in turn");

        // Nested: one user per call, its team recycled from the outer factory.
        let posts = Post::factory()
            .count(2)
            .for_factory(User::factory().for_factory(Team::factory(), "team"), "user")
            .recycle(&teams)
            .create(&db)
            .await
            .unwrap();
        assert_eq!(posts[0].user_id, posts[1].user_id);
        let owner = posts[0].user().first_or_fail(&db).await.unwrap();
        assert!(teams.iter().any(|t| Some(t.id) == owner.team_id));
        assert_eq!(Team::query().count(&db).await.unwrap(), before + 1);

        // The model's hooks run before the factory's callbacks, and
        // after_creating gets the executor; run it in a transaction here.
        let posts_before = Post::query().count(&db).await.unwrap();
        let tx = db.begin().await.unwrap();
        let note = Note::factory()
            .state(|n| n.log.push_str("state "))
            .after_making(|n| n.log.push_str("call-make "))
            .after_creating(|n, db| {
                Box::pin(async move {
                    n.log.push_str("call-create");
                    n.save(db).await
                })
            })
            .create_one(&tx)
            .await
            .unwrap();
        let order = "state hook-make call-make hook-create call-create";
        assert_eq!(note.log, order);
        assert_eq!(Note::find_or_fail(&tx, note.id).await.unwrap().log, order);
        let hooked = Post::query().where_("user_id", "=", note.id);
        let on_note = hooked.clone().count(&tx).await.unwrap();
        assert_eq!(Post::query().count(&tx).await.unwrap(), posts_before + 1);
        tx.rollback().await.unwrap();
        assert!(Note::find(&db, note.id).await.unwrap().is_none());
        assert_eq!(Post::query().count(&db).await.unwrap(), posts_before);
        assert_eq!(
            hooked.count(&db).await.unwrap() + 1,
            on_note,
            "the hook's post"
        );

        // The hook's future is `Send`, so `create` can run on a spawned task.
        let task_db = db.clone();
        let spawned = tokio::spawn(async move { Note::factory().create_one(&task_db).await })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(spawned.log, "hook-make hook-create ");
    }
}
