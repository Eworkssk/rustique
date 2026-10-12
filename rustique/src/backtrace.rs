//! Backtraces for error reports, trimmed to the frames that locate the
//! caller: the application's own code and the ORM call it made.

use std::backtrace::Backtrace;

/// Frames from these crates are runtime and plumbing; they never locate a bug.
const NOISE: &[&str] = &[
    "std::",
    "core::",
    "alloc::",
    "tokio::",
    "tracing::",
    "tracing_core::",
    "futures::",
    "futures_core::",
    "futures_util::",
    "mysql_async::",
    "mysql_common::",
    "oracle::",
    "test::",
    "fn(",
    "__rust",
    "rust_begin_unwind",
];

/// Frames kept at most; the outermost ones (runtime startup) matter least.
const MAX_FRAMES: usize = 32;

/// The current call stack, cleaned up by [`clean`]. Captured whatever
/// `RUST_BACKTRACE` says: callers only report errors that are bugs, so the
/// cost (milliseconds the first time, for symbols) is rare.
pub(crate) fn caller() -> String {
    clean(&Backtrace::force_capture().to_string())
}

/// Turns `std`'s backtrace text into one `symbol` line per frame, each with
/// its `at file:line` when the binary has debug info: runtime and driver
/// frames go, and each run of ORM frames keeps only its outermost frame, the
/// ORM call the application made.
///
/// shortcut: parses `Backtrace`'s `Display` text, whose format `std` does not
/// promise; switch to `Backtrace::frames` once it is stable.
fn clean(text: &str) -> String {
    // (symbol, location) per frame; an inlined function is a frame of its own.
    let mut frames: Vec<(&str, Option<&str>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(at) = line.strip_prefix("at ") {
            if let Some(frame) = frames.last_mut() {
                frame.1 = Some(at);
            }
        } else if !line.is_empty() {
            let symbol = line
                .split_once(": ")
                .filter(|(index, _)| index.bytes().all(|b| b.is_ascii_digit()))
                .map_or(line, |(_, symbol)| symbol);
            frames.push((symbol, None));
        }
    }
    frames.retain(|&(symbol, at)| !is_noise(symbol, at));
    let is_orm = |frame: Option<&(&str, _)>| {
        frame.is_some_and(|(s, _)| crate_of(s).starts_with("rustique::"))
    };
    let mut kept = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        if is_orm(Some(frame)) && is_orm(frames.get(index + 1)) {
            continue;
        }
        kept.push(match frame.1 {
            Some(at) => format!("{}\n    at {at}", frame.0),
            None => frame.0.to_owned(),
        });
        if kept.len() == MAX_FRAMES {
            break;
        }
    }
    kept.join("\n")
}

/// A frame of the runtime, a driver, the standard library (its sources are
/// under `/rustc/`) or C (`start_thread`; Rust symbols have a path).
fn is_noise(symbol: &str, at: Option<&str>) -> bool {
    at.is_some_and(|at| at.starts_with("/rustc/"))
        || !symbol.contains("::")
        || NOISE
            .iter()
            .any(|noise| crate_of(symbol).starts_with(noise))
}

/// The path a symbol starts with: `<app::User as rustique::Model>::from_row` is
/// the application's, `<core::pin::Pin<P> as ...>::poll` is `core`'s, and
/// `<F as core::future::IntoFuture>::into_future`, on a type parameter, the
/// trait's.
fn crate_of(symbol: &str) -> &str {
    let Some(symbol) = symbol.strip_prefix('<') else {
        return symbol;
    };
    match symbol.split_once(" as ") {
        Some((ty, rest)) if !ty.contains("::") => rest,
        _ => symbol,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_application_and_the_orm_call_it_made() {
        let text = "   0: std::backtrace::Backtrace::force_capture
             at /rustc/abc/library/std/src/backtrace.rs:312:9
   1: rustique::backtrace::caller
             at ./rustique/src/backtrace.rs:31:11
   2: rustique::row::Row::decode
             at ./rustique/src/row.rs:120:9
      rustique::row::Row::take_field
             at ./rustique/src/row.rs:98:15
   3: <app::models::User as rustique::model::Model>::from_row
             at ./src/models.rs:10:1
   4: rustique::model::hydrate
   5: <tracing::instrument::Instrumented<T> as core::future::future::Future>::poll
             at /home/u/.cargo/registry/src/tracing-0.1.41/src/instrument.rs:321:9
   6: rustique::model::query::Query<M>::get::{{closure}}
             at ./rustique/src/model/query.rs:774:9
   7: app::handlers::show::{{closure}}
             at ./src/handlers.rs:42:10
   8: <F as core::future::into_future::IntoFuture>::into_future
   9: tokio::runtime::task::core::Core<T,S>::poll
  10: <fn() as core::ops::function::FnOnce<()>>::call_once
             at /rustc/abc/library/core/src/ops/function.rs:250:5
  11: __rust_begin_short_backtrace
  12: start_thread
             at ./nptl/pthread_create.c:447:8
";
        assert_eq!(
            clean(text),
            "rustique::row::Row::take_field
    at ./rustique/src/row.rs:98:15
<app::models::User as rustique::model::Model>::from_row
    at ./src/models.rs:10:1
rustique::model::query::Query<M>::get::{{closure}}
    at ./rustique/src/model/query.rs:774:9
app::handlers::show::{{closure}}
    at ./src/handlers.rs:42:10"
        );
    }

    #[test]
    fn captures_this_test() {
        assert!(caller().contains("captures_this_test"), "{}", caller());
    }
}
