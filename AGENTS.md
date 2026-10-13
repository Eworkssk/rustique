# Notes for coding agents

- Using the ORM in an app: read `rustique/docs/README.md` first (cheat sheet plus links). Every page there matches the current API.
- Public API: `rustique/src/lib.rs` (re-exports). Macros: `rustique-macros/src/lib.rs`.
- Tests: `rustique/tests/sql/` need no database; `rustique/tests/db/` run only with `RUSTIQUE_TEST_MYSQL_URL` / `RUSTIQUE_TEST_ORACLE_URL` set (see `docker-compose.yml`). The code blocks in `rustique/docs/` compile as doctests.
- Before finishing: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features` (no warnings) and `cargo test --workspace --all-features` with both database URLs set.
- When you change public behaviour, update the matching page in `rustique/docs/`.
- A new public `Query` method must also go into `rustique-macros/src/query_methods.txt`, so scopes cannot be hidden by it; the `query_methods` test in `rustique/tests/sql/` fails when the file and `Query` differ.
