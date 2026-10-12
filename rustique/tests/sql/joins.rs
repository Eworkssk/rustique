use rustique::{Dialect, QueryBuilder};

use crate::{assert_sql, error};

#[test]
fn inner_and_left_joins() {
    let query = QueryBuilder::table("users")
        .join("posts", "posts.user_id", "=", "users.id")
        .left_join("roles as r", "r.id", "=", "users.role_id");
    assert_sql(
        &query,
        "SELECT * FROM `users` INNER JOIN `posts` ON `posts`.`user_id` = `users`.`id` \
         LEFT JOIN `roles` AS `r` ON `r`.`id` = `users`.`role_id`",
        r#"SELECT * FROM "USERS" INNER JOIN "POSTS" ON "POSTS"."USER_ID" = "USERS"."ID" LEFT JOIN "ROLES" "R" ON "R"."ID" = "USERS"."ROLE_ID""#,
    );
}

#[test]
fn join_bindings_come_before_where_bindings() {
    let query = QueryBuilder::table("users")
        .join_with("posts", |j| {
            j.on("posts.user_id", "=", "users.id")
                .where_("posts.published", "=", true)
                .or_on("posts.editor_id", "=", "users.id")
        })
        .where_("users.active", "=", 1);
    assert_sql(
        &query,
        "SELECT * FROM `users` INNER JOIN `posts` ON `posts`.`user_id` = `users`.`id` AND `posts`.`published` = ? \
         OR `posts`.`editor_id` = `users`.`id` WHERE `users`.`active` = ?",
        r#"SELECT * FROM "USERS" INNER JOIN "POSTS" ON "POSTS"."USER_ID" = "USERS"."ID" AND "POSTS"."PUBLISHED" = :1 OR "POSTS"."EDITOR_ID" = "USERS"."ID" WHERE "USERS"."ACTIVE" = :2"#,
    );
}

#[test]
fn right_and_cross_joins() {
    let query = QueryBuilder::table("a")
        .right_join("b", "b.a_id", "=", "a.id")
        .cross_join("c");
    assert_sql(
        &query,
        "SELECT * FROM `a` RIGHT JOIN `b` ON `b`.`a_id` = `a`.`id` CROSS JOIN `c`",
        r#"SELECT * FROM "A" RIGHT JOIN "B" ON "B"."A_ID" = "A"."ID" CROSS JOIN "C""#,
    );
}

#[test]
fn join_without_on_is_an_error() {
    let query = QueryBuilder::table("a").join_with("b", |j| j);
    assert!(error(&query, Dialect::Oracle).contains("no ON condition"));
}

#[test]
fn nested_and_raw_join_conditions() {
    // Inner join of a price list on products matched by key or by a type prefix.
    let query = QueryBuilder::table("price_list as list")
        .join_with("products", |j| {
            j.where_group(|g| g.where_("kod_id", "<>", 0).on("sk_id", "=", "kod_id"))
                .or_where_group(|g| {
                    g.where_("id_tree_str", "<>", "0")
                        .where_raw("typ_t LIKE id_tree_str || ?", ["%".into()])
                })
        })
        .where_("list.active", "=", 1);
    assert_sql(
        &query,
        "SELECT * FROM `price_list` AS `list` INNER JOIN `products` ON (`kod_id` <> ? AND `sk_id` = `kod_id`) \
         OR (`id_tree_str` <> ? AND (typ_t LIKE id_tree_str || ?)) WHERE `list`.`active` = ?",
        r#"SELECT * FROM "PRICE_LIST" "LIST" INNER JOIN "PRODUCTS" ON ("KOD_ID" <> :1 AND "SK_ID" = "KOD_ID") OR ("ID_TREE_STR" <> :2 AND (typ_t LIKE id_tree_str || :3)) WHERE "LIST"."ACTIVE" = :4"#,
    );
}

#[test]
fn subqueries_as_join_and_from_sources() {
    let hashes = QueryBuilder::table("previews")
        .select(["vp_id"])
        .select_raw("MAX(session_id) AS hash", [])
        .where_("kind", "=", "ca")
        .group_by(["vp_id"]);
    let query = QueryBuilder::table("carts")
        .left_join_sub(hashes, "preview", "preview.vp_id", "=", "carts.vp_id")
        .where_("carts.id", ">", 5);
    assert_sql(
        &query,
        "SELECT * FROM `carts` LEFT JOIN (SELECT `vp_id`, MAX(session_id) AS hash FROM `previews` WHERE `kind` = ? GROUP BY `vp_id`) `preview` \
         ON `preview`.`vp_id` = `carts`.`vp_id` WHERE `carts`.`id` > ?",
        r#"SELECT * FROM "CARTS" LEFT JOIN (SELECT "VP_ID", MAX(session_id) AS hash FROM "PREVIEWS" WHERE "KIND" = :1 GROUP BY "VP_ID") "PREVIEW" ON "PREVIEW"."VP_ID" = "CARTS"."VP_ID" WHERE "CARTS"."ID" > :2"#,
    );

    let grouped = QueryBuilder::table("orders")
        .select_raw("branch, COUNT(*) AS n", [])
        .where_("fb", "=", 3)
        .group_by_raw("branch", []);
    let query = QueryBuilder::from_sub(grouped, "per_branch")
        .where_("n", ">", 1)
        .limit(5);
    assert_sql(
        &query,
        "SELECT * FROM (SELECT branch, COUNT(*) AS n FROM `orders` WHERE `fb` = ? GROUP BY branch) `per_branch` WHERE `n` > ? LIMIT 5",
        r#"SELECT * FROM (SELECT branch, COUNT(*) AS n FROM "ORDERS" WHERE "FB" = :1 GROUP BY branch) "PER_BRANCH" WHERE "N" > :2 FETCH NEXT 5 ROWS ONLY"#,
    );
    let delete = QueryBuilder::from_sub(QueryBuilder::table("a"), "t");
    assert!(
        delete
            .to_delete_sql(Dialect::MySql)
            .unwrap_err()
            .to_string()
            .contains("subqueries in FROM")
    );
}

#[test]
fn right_join_with_several_conditions() {
    let query = QueryBuilder::table("sk").right_join_with("types as tt", |j| {
        j.on("sk.typ_t", "=", "tt.id")
            .on("stpo.rid_o", "=", "tt.rid_stp")
    });
    assert_sql(
        &query,
        "SELECT * FROM `sk` RIGHT JOIN `types` AS `tt` ON `sk`.`typ_t` = `tt`.`id` AND `stpo`.`rid_o` = `tt`.`rid_stp`",
        r#"SELECT * FROM "SK" RIGHT JOIN "TYPES" "TT" ON "SK"."TYP_T" = "TT"."ID" AND "STPO"."RID_O" = "TT"."RID_STP""#,
    );
}
