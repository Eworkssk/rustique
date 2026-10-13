//! `rustique-macros/src/query_methods.txt` must list every public `Query` method.

use std::collections::BTreeSet;

#[test]
fn macro_list_matches_query() {
    let listed: BTreeSet<&str> = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rustique-macros/src/query_methods.txt"
    ))
    .lines()
    .filter(|l| !l.is_empty())
    .collect();

    // Scan the `impl<M: Model> Query<M>` block: `pub fn` lines plus `delegate! { name(..); }` entries.
    let (mut in_impl, mut in_delegate) = (false, false);
    let mut actual = BTreeSet::new();
    for line in include_str!("../../src/model/query.rs").lines() {
        if line.starts_with("impl") {
            in_impl = line.contains("> Query<M> {");
        } else if !in_impl {
            continue;
        } else if line.starts_with("    delegate! {") {
            in_delegate = true;
        } else if in_delegate && line == "    }" {
            in_delegate = false;
        } else if in_delegate {
            actual.extend(line.trim().split('(').next().filter(|n| !n.is_empty()));
        } else if let Some(rest) = line.strip_prefix("    pub ") {
            let rest = rest
                .trim_start_matches("async ")
                .trim_start_matches("const ");
            if let Some(rest) = rest.strip_prefix("fn ") {
                actual.extend(rest.split(['(', '<']).next());
            }
        }
    }

    let missing: Vec<_> = actual.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(&actual).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "query_methods.txt: missing {missing:?}, extra {extra:?}"
    );
}
