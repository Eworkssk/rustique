//! Relation `scope`, `latest_of_many` / `oldest_of_many`, `with("rel:cols")`
//! and models read from partial selects.

use chrono::NaiveDateTime;
use rustique::{Executor, Model, Query, Rel, Schema};

use crate::databases;

#[rustique::model(table = "orm_rc_authors")]
#[derive(Debug, Clone, Default)]
struct Author {
    id: i64,
    name: String,
    #[rustique(has_many(Article, foreign_key = "author_id", scope = published))]
    published_articles: Rel<Vec<Article>>,
    #[rustique(has_one(Article, foreign_key = "author_id", latest_of_many))]
    latest_article: Rel<Option<Article>>,
    #[rustique(has_one(Article, foreign_key = "author_id", oldest_of_many = "published_at", scope = published))]
    first_published: Rel<Option<Article>>,
    #[rustique(has_many(Article, foreign_key = "author_id"))]
    articles: Rel<Vec<Article>>,
    #[rustique(count)]
    published_articles_count: Rel<u64>,
}

#[rustique::model(table = "orm_rc_articles")]
#[derive(Debug, Clone, Default)]
struct Article {
    id: i64,
    author_id: i64,
    title: String,
    published: bool,
    published_at: Option<NaiveDateTime>,
    #[rustique(belongs_to(Author, foreign_key = "author_id"))]
    author: Rel<Option<Box<Author>>>,
}

fn published(q: Query<Article>) -> Query<Article> {
    q.where_("orm_rc_articles.published", "=", true)
}

fn at(day: u32) -> Option<NaiveDateTime> {
    chrono::NaiveDate::from_ymd_opt(2026, 1, day)?.and_hms_opt(0, 0, 0)
}

#[tokio::test]
async fn relation_constraints() {
    for db in databases().await {
        let dialect = db.dialect();
        for table in ["orm_rc_articles", "orm_rc_authors"] {
            Schema::drop_if_exists(&db, table).await.unwrap();
        }
        Schema::create(&db, "orm_rc_authors", |t| {
            t.id();
            t.string("name", 50);
        })
        .await
        .unwrap();
        Schema::create(&db, "orm_rc_articles", |t| {
            t.id();
            t.big_integer("author_id");
            t.string("title", 50);
            t.boolean("published");
            t.datetime("published_at").nullable();
        })
        .await
        .unwrap();

        for name in ["ann", "bob", "cy"] {
            let mut author = Author {
                name: name.into(),
                ..Default::default()
            };
            author.save(&db).await.unwrap();
        }
        // (author, title, published, published_at); ids follow this order.
        let rows = [
            (1, "a1", true, at(3)),
            (1, "a2", true, at(1)),
            (1, "a3", false, None),
            (2, "b1", false, at(2)),
            (2, "b2", true, None),
        ];
        for (author_id, title, published, published_at) in rows {
            let mut article = Article {
                author_id,
                title: title.into(),
                published,
                published_at,
                ..Default::default()
            };
            article.save(&db).await.unwrap();
        }

        let authors = Author::query()
            .with("published_articles")
            .with("latest_article")
            .with("first_published")
            .with_count("published_articles")
            .order_by("id")
            .get(&db)
            .await
            .unwrap_or_else(|e| panic!("{dialect}: {e}"));
        let titles = |articles: &Vec<Article>| {
            let mut titles: Vec<_> = articles.iter().map(|a| a.title.clone()).collect();
            titles.sort();
            titles
        };
        let title = |article: &Option<Article>| article.as_ref().map(|a| a.title.clone());
        let [ann, bob, cy] = &authors[..] else {
            panic!("three authors")
        };
        assert_eq!(
            titles(ann.published_articles.loaded().unwrap()),
            ["a1", "a2"]
        );
        assert_eq!(titles(bob.published_articles.loaded().unwrap()), ["b2"]);
        assert_eq!(*ann.published_articles_count.loaded().unwrap(), 2);
        assert_eq!(
            title(ann.latest_article.loaded().unwrap()).as_deref(),
            Some("a3")
        );
        assert_eq!(
            title(bob.latest_article.loaded().unwrap()).as_deref(),
            Some("b2")
        );
        assert_eq!(title(cy.latest_article.loaded().unwrap()), None);
        // Oldest published: a2 (Jan 1); bob's only published row has no date.
        assert_eq!(
            title(ann.first_published.loaded().unwrap()).as_deref(),
            Some("a2")
        );
        assert_eq!(title(bob.first_published.loaded().unwrap()), None);

        // Lazy queries and `where_has` agree.
        let latest = ann.latest_article().get(&db).await.unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].title, "a3");
        let with_dated_first = Author::query()
            .where_has("first_published", |q| q)
            .pluck::<String>(&db, "name")
            .await
            .unwrap();
        assert_eq!(with_dated_first, ["ann"], "{dialect}");

        // `with("rel:cols")`: only those columns; the rest take their default.
        let authors = Author::query()
            .with("articles:id,title")
            .with("articles.author:id")
            .order_by("id")
            .get(&db)
            .await
            .unwrap_or_else(|e| panic!("{dialect}: {e}"));
        let articles = authors[0].articles.loaded().unwrap();
        assert_eq!(articles.len(), 3);
        assert!(
            articles
                .iter()
                .all(|a| !a.title.is_empty() && !a.published && a.author_id == 0)
        );
        // Nested relations need their keys selected: `author_id` was not.
        assert!(articles[0].author.loaded().unwrap().is_none());

        let parents = Article::query()
            .with("author:id,name")
            .order_by("id")
            .get(&db)
            .await
            .unwrap();
        let author = parents[0].author.loaded().unwrap().as_ref().unwrap();
        assert_eq!((author.id, author.name.as_str()), (1, "ann"));

        // A partial select on the model itself.
        let names = Author::query()
            .select(["name"])
            .order_by("id")
            .get(&db)
            .await
            .unwrap_or_else(|e| panic!("{dialect}: {e}"));
        assert_eq!(names[1].name, "bob");
        assert_eq!(names[1].id, 0);
    }
}
