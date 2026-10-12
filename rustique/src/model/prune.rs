//! [`Prunable`]: periodic deletion of obsolete models, like Eloquent's
//! `Prunable` / `MassPrunable` traits.

use std::future::Future;

use super::{Model, Query};
use crate::Result;
use crate::db::Executor;

/// A model whose obsolete rows can be deleted with [`prune`](Self::prune),
/// like Eloquent's `Prunable`; call it from the application's scheduler.
///
/// ```
/// use rustique::prelude::*;
/// use rustique::{Prunable, Query};
///
/// #[rustique::model(table = "sessions")]
/// #[derive(Debug, Clone, Default)]
/// pub struct Session {
///     pub id: i64,
///     pub last_seen: chrono::NaiveDateTime,
/// }
///
/// impl Prunable for Session {
///     fn prunable() -> Query<Self> {
///         let cutoff = rustique::now() - chrono::Duration::days(30);
///         Session::query().where_("last_seen", "<", cutoff)
///     }
/// }
///
/// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
/// let deleted = Session::prune(db).await?;
/// # Ok(()) }
/// ```
pub trait Prunable: Model {
    /// The models to delete. Soft-deleted rows are included.
    fn prunable() -> Query<Self>;

    /// Runs before each model is deleted by [`prune`](Self::prune), e.g. to
    /// remove its files. An `Err` stops pruning.
    fn pruning(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        let _ = db;
        async { Ok(()) }
    }

    /// Deletes every [`prunable`](Self::prunable) model for real, one at a
    /// time in chunks of 1000 by primary key, running
    /// [`pruning`](Self::pruning) and the `force_delete` [`Hooks`](crate::Hooks)
    /// of each. Returns how many were deleted.
    ///
    /// # Errors
    ///
    /// Hook and database errors; models deleted before stay deleted.
    fn prune(db: &impl Executor) -> impl Future<Output = Result<u64>> + Send {
        async move {
            let mut chunks = Self::prunable().with_trashed().chunk_by_id(1000);
            let mut count = 0;
            while let Some(models) = chunks.next(db).await? {
                for mut model in models {
                    model.pruning(db).await?;
                    model.force_delete(db).await?;
                    count += 1;
                }
            }
            Ok(count)
        }
    }

    /// Deletes every [`prunable`](Self::prunable) row with one `DELETE`,
    /// like Eloquent's `MassPrunable`: no models are loaded and no hooks
    /// run. Returns how many rows were deleted.
    ///
    /// # Errors
    ///
    /// Database errors.
    fn mass_prune(db: &impl Executor) -> impl Future<Output = Result<u64>> + Send {
        async move { Self::prunable().with_trashed().force_delete(db).await }
    }
}
