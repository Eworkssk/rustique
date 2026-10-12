//! [`Cursor`]: rows streamed one at a time from a background task.

use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Span;

use crate::{Result, Row};

/// Rows read ahead of the consumer unless the query sets its own; about one
/// network fetch.
pub(crate) const BUFFER: usize = 100;

/// Where a driver task sends rows; a send fails once the [`Cursor`] is dropped.
pub(super) type Sink = mpsc::Sender<Result<Row>>;

/// Rows of a query, streamed instead of collected, from
/// [`QueryBuilder::cursor`](crate::QueryBuilder::cursor). Call
/// [`next`](Self::next) until it returns `None`.
///
/// A background task reads the rows on a connection of its own (or the
/// transaction's, which waits for the cursor) and keeps at most a hundred
/// ahead (see [`QueryBuilder::cursor_buffered`](crate::QueryBuilder::cursor_buffered)),
/// so memory stays flat whatever the result size. Dropping the cursor stops
/// the query.
///
/// A cursor on a [`Transaction`](crate::Transaction) holds its connection:
/// until the cursor returns `None` or is dropped, other statements on that
/// transaction fail with [`Error::InvalidQuery`](crate::Error::InvalidQuery)
/// (waiting would deadlock once the read-ahead buffer is full).
#[must_use]
#[derive(Debug)]
pub struct Cursor<'a> {
    rows: mpsc::Receiver<Result<Row>>,
    task: Option<JoinHandle<()>>,
    /// The transaction's "cursor open" flag, cleared when this cursor ends.
    /// Also borrows the executor, so a transaction cannot commit under a cursor.
    open: Option<&'a AtomicBool>,
    executor: PhantomData<&'a ()>,
}

impl<'a> Cursor<'a> {
    /// Runs `stream` on the async runtime. Its error, if any, becomes the last item.
    /// `buffer`: rows read ahead, at least 1.
    pub(super) fn spawn<F>(buffer: usize, stream: impl FnOnce(Sink) -> F) -> Self
    where
        F: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        use tracing::Instrument;

        let (sink, rows) = mpsc::channel(buffer.max(1));
        let errors = sink.clone();
        let stream = stream(sink);
        let task = tokio::spawn(
            async move {
                if let Err(error) = stream.await {
                    super::log::fail(&Span::current(), &error);
                    let _ = errors.send(Err(error)).await; // the cursor may be gone
                }
            }
            // The query's span, entered by the caller; it lasts as long as the rows.
            .instrument(Span::current()),
        );
        Self::new(rows, task)
    }

    #[cfg(feature = "oracle")]
    /// Runs blocking driver code `stream` on tokio's blocking thread pool.
    pub(super) fn spawn_blocking(
        buffer: usize,
        stream: impl FnOnce(&Sink) -> Result<()> + Send + 'static,
    ) -> Self {
        let (sink, rows) = mpsc::channel(buffer.max(1));
        let span = Span::current(); // as in `spawn`
        let task = tokio::task::spawn_blocking(move || {
            let span = span.entered();
            if let Err(error) = stream(&sink) {
                super::log::fail(&span, &error);
                let _ = sink.blocking_send(Err(error));
            }
        });
        Self::new(rows, task)
    }

    fn new(rows: mpsc::Receiver<Result<Row>>, task: JoinHandle<()>) -> Self {
        Self {
            rows,
            task: Some(task),
            open: None,
            executor: PhantomData,
        }
    }

    /// A cursor whose only item is `error`.
    pub(super) fn failed(error: crate::Error) -> Self {
        let (sink, rows) = mpsc::channel(1);
        sink.try_send(Err(error)).expect("the channel has room");
        Self {
            rows,
            task: None,
            open: None,
            executor: PhantomData,
        }
    }

    /// Clears `open` when this cursor ends.
    pub(super) fn hold(&mut self, open: &'a AtomicBool) {
        self.open = Some(open);
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            open.store(false, Ordering::Release);
        }
    }

    /// The next row, or `None` after the last one or an error.
    ///
    /// # Errors
    ///
    /// Database and decode errors, as for [`Executor::fetch_all`](crate::Executor::fetch_all).
    pub async fn next(&mut self) -> Result<Option<Row>> {
        if let Some(row) = self.rows.recv().await {
            if row.is_err() {
                self.close(); // the driver task has finished and freed the connection
            }
            return row.map(Some);
        }
        self.close();
        // A panicking driver task closes the channel too; report it rather
        // than end the rows early.
        if let Some(task) = self.task.take()
            && let Err(error) = task.await
            && error.is_panic()
        {
            std::panic::resume_unwind(error.into_panic());
        }
        Ok(None)
    }
}

impl Drop for Cursor<'_> {
    fn drop(&mut self) {
        // The driver task stops at its next row and frees the connection;
        // a statement sent meanwhile waits for it, briefly.
        self.close();
    }
}
