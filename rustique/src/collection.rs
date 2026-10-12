//! [`Collection`]: the `Vec` returned by queries that fetch many rows.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::{Deref, DerefMut};

use itertools::Itertools;

use crate::model::{EagerLoad, eager_load};
use crate::{Executor, Model, Result};

/// Rows or models returned by a query, like Laravel's `Collection`.
///
/// Derefs to `Vec<T>`, so every `Vec` and slice method works, and
/// [`Itertools`] (re-exported in the prelude) adds the rest on `.iter()`.
/// Use [`into_inner`](Self::into_inner) or `Vec::from` to get the `Vec` back.
///
/// ```
/// use rustique::Collection;
///
/// let users = Collection::from(vec![("ann", 1), ("bo", 2), ("cy", 1)]);
/// let names = users.pluck(|u| u.0);
/// let by_team = users.group_by(|u| u.1);
/// assert_eq!(names, ["ann", "bo", "cy"]);
/// assert_eq!(by_team[&1].len(), 2);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Collection<T>(Vec<T>);

impl<T> Collection<T> {
    /// The underlying `Vec`.
    #[must_use]
    pub fn into_inner(self) -> Vec<T> {
        self.0
    }

    /// One value from each item, in order.
    #[must_use]
    pub fn pluck<U>(&self, f: impl FnMut(&T) -> U) -> Collection<U> {
        self.0.iter().map(f).collect()
    }

    /// Items by key; on duplicate keys the last item wins.
    #[must_use]
    pub fn key_by<K: Hash + Eq>(&self, mut f: impl FnMut(&T) -> K) -> HashMap<K, &T> {
        self.0.iter().map(|item| (f(item), item)).collect()
    }

    /// Items grouped by key, keeping their order within each group.
    #[must_use]
    pub fn group_by<K: Hash + Eq>(&self, mut f: impl FnMut(&T) -> K) -> HashMap<K, Vec<&T>> {
        self.0.iter().into_group_map_by(|item| f(item))
    }
}

impl<M: Model> Collection<M> {
    /// Loads relation paths (`"posts.comments"`) into every model's [`Rel`]
    /// fields, like Laravel's `$collection->load()`: one query per relation
    /// for the whole collection, where [`Model::load`] in a loop would run
    /// one per model.
    ///
    /// [`Rel`]: crate::Rel
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRelation`](crate::Error::UnknownRelation), database
    /// and decode errors.
    pub async fn load(&mut self, db: &impl Executor, relations: &str) -> Result<()> {
        eager_load(db, &mut self.0, &[EagerLoad::path(relations)]).await
    }
}

impl<T> Deref for Collection<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> DerefMut for Collection<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T: PartialEq<U>, U, const N: usize> PartialEq<[U; N]> for Collection<T> {
    fn eq(&self, other: &[U; N]) -> bool {
        self.0 == other
    }
}

impl<T: PartialEq<U>, U> PartialEq<[U]> for Collection<T> {
    fn eq(&self, other: &[U]) -> bool {
        self.0 == other
    }
}

impl<T: PartialEq<U>, U> PartialEq<Vec<U>> for Collection<T> {
    fn eq(&self, other: &Vec<U>) -> bool {
        self.0 == *other
    }
}

impl<T> From<Vec<T>> for Collection<T> {
    fn from(items: Vec<T>) -> Self {
        Self(items)
    }
}

impl<T> From<Collection<T>> for Vec<T> {
    fn from(items: Collection<T>) -> Self {
        items.0
    }
}

impl<T> FromIterator<T> for Collection<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self(Vec::from_iter(iter))
    }
}

impl<T> IntoIterator for Collection<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a Collection<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Collection<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}
