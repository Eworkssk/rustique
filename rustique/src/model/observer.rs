//! [`Observer`]s: lifecycle hooks kept outside the model, and the dispatch
//! of every lifecycle [`Event`] to [`Hooks`](super::Hooks) and observers.

use std::future::Future;

use super::Model;
use crate::Result;
use crate::db::Executor;

/// Lifecycle hooks for model `M` kept in a type of their own, like Eloquent
/// observers. Register them on the model with
/// `#[rustique::model(observers(Audit, ...))]`; they run in that order after the
/// model's own [`Hooks`](super::Hooks).
///
/// Every method does nothing by default; one that returns `Err` aborts the
/// operation. One observer can serve every model:
///
/// ```
/// use rustique::prelude::*;
///
/// struct Audit;
///
/// impl<M: Model> rustique::Observer<M> for Audit {
///     async fn deleted(model: &mut M, db: &impl Executor) -> rustique::Result<()> {
///         let _ = (model, db); // write an audit row, ...
///         Ok(())
///     }
/// }
///
/// #[rustique::model(table = "users", observers(Audit))]
/// #[derive(Debug, Clone, Default)]
/// pub struct User {
///     pub id: i64,
/// }
/// ```
///
/// Observers have no `self`: keep shared state in a `static` if needed.
#[allow(unused_variables)]
pub trait Observer<M: Model> {
    /// Before an insert or update.
    fn saving(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an insert or update, and after a `save` with nothing to update.
    fn saved(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before an insert.
    fn creating(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an insert; the generated key is set.
    fn created(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before an update of changed columns.
    fn updating(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an update.
    fn updated(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before a delete or soft delete.
    fn deleting(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After a delete or soft delete.
    fn deleted(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before [`Model::force_delete`], ahead of `deleting`.
    fn force_deleting(
        model: &mut M,
        db: &impl Executor,
    ) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After [`Model::force_delete`], following `deleted`.
    fn force_deleted(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before [`Model::restore`], ahead of `saving`.
    fn restoring(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After [`Model::restore`], following `saved`.
    fn restored(model: &mut M, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

/// A lifecycle event: one method of [`Hooks`](super::Hooks) and [`Observer`].
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Saving,
    Saved,
    Creating,
    Created,
    Updating,
    Updated,
    Deleting,
    Deleted,
    ForceDeleting,
    ForceDeleted,
    Restoring,
    Restored,
}

/// Runs `event` on the model's [`Hooks`](super::Hooks), then on its observers.
pub(super) async fn fire<M: Model>(model: &mut M, event: Event, db: &impl Executor) -> Result<()> {
    match event {
        Event::Saving => model.saving(db).await,
        Event::Saved => model.saved(db).await,
        Event::Creating => model.creating(db).await,
        Event::Created => model.created(db).await,
        Event::Updating => model.updating(db).await,
        Event::Updated => model.updated(db).await,
        Event::Deleting => model.deleting(db).await,
        Event::Deleted => model.deleted(db).await,
        Event::ForceDeleting => model.force_deleting(db).await,
        Event::ForceDeleted => model.force_deleted(db).await,
        Event::Restoring => model.restoring(db).await,
        Event::Restored => model.restored(db).await,
    }?;
    model.fire_observers(event, db).await
}

/// Runs `event` on observer `O`; called by `#[rustique::model(observers(...))]`.
#[doc(hidden)]
pub async fn observe<M: Model, O: Observer<M>>(
    model: &mut M,
    event: Event,
    db: &impl Executor,
) -> Result<()> {
    match event {
        Event::Saving => O::saving(model, db).await,
        Event::Saved => O::saved(model, db).await,
        Event::Creating => O::creating(model, db).await,
        Event::Created => O::created(model, db).await,
        Event::Updating => O::updating(model, db).await,
        Event::Updated => O::updated(model, db).await,
        Event::Deleting => O::deleting(model, db).await,
        Event::Deleted => O::deleted(model, db).await,
        Event::ForceDeleting => O::force_deleting(model, db).await,
        Event::ForceDeleted => O::force_deleted(model, db).await,
        Event::Restoring => O::restoring(model, db).await,
        Event::Restored => O::restored(model, db).await,
    }
}
