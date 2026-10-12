//! Model factories for test and seed data, like Laravel's `HasFactory`.

use std::any::TypeId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::relation::{Pivot, Relation, RelationKind, column_values};
use super::{BoxFuture, Model, column_index};
use crate::db::{AnyExecutor, Executor};
use crate::{Collection, Result, Value};

/// Next `n` passed to [`HasFactory::definition`], shared by all models.
static SEQUENCE: AtomicU64 = AtomicU64::new(1);
/// Where each `make` / `create` call starts in the recycled models, which
/// its models then take in turn.
static RECYCLE: AtomicUsize = AtomicUsize::new(0);

/// A model with default attributes for tests and seeders, like Laravel's
/// `HasFactory` plus the factory's `definition()`.
///
/// ```
/// use rustique::prelude::*;
///
/// #[rustique::model(table = "users")]
/// #[derive(Debug, Clone, Default)]
/// pub struct User {
///     pub id: i64,
///     pub name: String,
///     pub email: String,
///     pub admin: bool,
/// }
///
/// impl HasFactory for User {
///     fn definition(n: u64) -> Self {
///         User { name: format!("User {n}"), email: format!("user{n}@example.com"), ..Default::default() }
///     }
/// }
///
/// let admins = User::factory().count(2).state(|u| u.admin = true).make();
/// assert!(admins.iter().all(|u| u.admin && !u.exists()));
/// assert_ne!(admins[0].email, admins[1].email);
/// ```
pub trait HasFactory: Model {
    /// A new, unsaved model with default attributes. `n` is different on
    /// every call in the process (1, 2, 3, ...), for unique columns.
    fn definition(n: u64) -> Self;

    /// A [`Factory`] making one model; see its methods.
    fn factory() -> Factory<Self> {
        Factory {
            count: 1,
            parents: Vec::new(),
            states: Vec::new(),
            after_making: Vec::new(),
            children: Vec::new(),
            after_creating: Vec::new(),
            recycled: Vec::new(),
        }
    }
}

/// Column values to set on a model.
type Values = Vec<(&'static str, Value)>;
type State<M> = Box<dyn Fn(&mut M, usize) + Send + Sync>;
type AfterMaking<M> = Box<dyn Fn(&mut M) + Send + Sync>;
type AfterCreating<M> =
    Box<dyn for<'a> Fn(&'a mut M, &'a AnyExecutor<'a>) -> BoxFuture<'a, Result<()>> + Send + Sync>;
/// Models given to [`Factory::recycle`]: their type and their rows, each
/// in `COLUMNS` order.
type Recycled = (TypeId, Arc<[Vec<Value>]>);

/// Builds models from [`HasFactory::definition`] plus states, from
/// `Model::factory()`. Reusable: every `make` / `create` builds new models.
///
/// For each model, in order: `definition`, the parents' keys
/// ([`for_`](Self::for_)), the [`state`](Self::state)s, the keys of the
/// parent that [`has`](Self::has) creates it for, the
/// [`after_making`](Self::after_making) callbacks. `create` then saves it,
/// creates its [`has`](Self::has) children and, once all models are
/// saved, runs the [`after_creating`](Self::after_creating) callbacks.
#[must_use]
pub struct Factory<M> {
    count: usize,
    parents: Vec<Parent>,
    states: Vec<State<M>>,
    after_making: Vec<AfterMaking<M>>,
    children: Vec<Box<dyn Children<M>>>,
    after_creating: Vec<AfterCreating<M>>,
    recycled: Vec<Recycled>,
}

impl<M: HasFactory> Factory<M> {
    /// How many models [`make`](Self::make) and [`create`](Self::create) build.
    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    /// Changes every model after its definition, like Laravel's `state`.
    /// States run in the order they were added.
    pub fn state(self, state: impl Fn(&mut M) + Send + Sync + 'static) -> Self {
        self.sequence(move |model, _| state(model))
    }

    /// As [`state`](Self::state), also given the model's index in the
    /// batch (from 0), like Laravel's `sequence`:
    /// `.sequence(|u, i| u.admin = i % 2 == 0)` alternates.
    pub fn sequence(mut self, state: impl Fn(&mut M, usize) + Send + Sync + 'static) -> Self {
        self.states.push(Box::new(state));
        self
    }

    /// Runs `callback` on every model once it is made, after all states,
    /// like Laravel's `afterMaking`.
    pub fn after_making(mut self, callback: impl Fn(&mut M) + Send + Sync + 'static) -> Self {
        self.after_making.push(Box::new(callback));
        self
    }

    /// Runs `callback` on every model [`create`](Self::create) saved, with
    /// the executor it was given, like Laravel's `afterCreating`:
    ///
    /// ```
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "users")]
    /// # #[derive(Debug, Clone, Default)]
    /// # pub struct User { pub id: i64, pub name: String }
    /// # impl HasFactory for User { fn definition(n: u64) -> Self { User::default() } }
    /// let factory = User::factory().after_creating(|user, db| Box::pin(async move {
    ///     user.name = format!("User {}", user.id);
    ///     user.save(db).await
    /// }));
    /// ```
    pub fn after_creating<F>(mut self, callback: F) -> Self
    where
        F: for<'a> Fn(&'a mut M, &'a AnyExecutor<'a>) -> BoxFuture<'a, Result<()>>
            + Send
            + Sync
            + 'static,
    {
        self.after_creating.push(Box::new(callback));
        self
    }

    /// Makes every model belong to `parent` through `relation`, a
    /// `belongs_to` or `morph_to` relation of `M`, like Laravel's
    /// `for($user)`: sets the foreign key (and the morph type).
    ///
    /// # Panics
    ///
    /// When `M` has no such relation to `P`.
    pub fn for_<P: Model>(mut self, parent: &P, relation: &str) -> Self {
        let keys = ParentKeys::new::<M, P>(relation);
        let values = keys.values::<P>(&parent.to_values());
        self.parents.push(Parent::Values(values));
        self
    }

    /// As [`for_`](Self::for_), with a parent that `parent` creates, like
    /// Laravel's `for(User::factory())`. Each [`create`](Self::create)
    /// call creates one parent for all its models, unless
    /// [`recycle`](Self::recycle) provides `P` models; then each model gets
    /// one of those. [`make`](Self::make) creates nothing: it uses recycled
    /// models or leaves the foreign key as the definition set it.
    ///
    /// # Panics
    ///
    /// When `M` has no such relation to `P`.
    pub fn for_factory<P: HasFactory>(mut self, parent: Factory<P>, relation: &str) -> Self {
        let keys = ParentKeys::new::<M, P>(relation);
        let parent = ForFactory {
            factory: parent,
            keys,
        };
        self.parents.push(Parent::Factory(Box::new(parent)));
        self
    }

    /// After saving each model, [`create`](Self::create) creates the
    /// models of `children` related to it through `relation`, like
    /// Laravel's `has(Post::factory()->count(3), 'posts')`. `relation` is a
    /// `has_one`, `has_many`, `morph_one`, `morph_many` (the foreign key and
    /// morph type are set) or `belongs_to_many` (pivot rows are inserted)
    /// relation of `M`.
    ///
    /// # Panics
    ///
    /// When `M` has no such relation to `R`.
    pub fn has<R: HasFactory>(self, children: Factory<R>, relation: &str) -> Self {
        let relation_info = child_relation::<M, R>(relation);
        self.push_children(children, relation, relation_info, Vec::new())
    }

    /// As [`has`](Self::has) for a `belongs_to_many` relation, setting the
    /// extra pivot columns `pivot` on every pivot row, like Laravel's
    /// `hasAttached`. Pivot timestamps are filled as by `attach`.
    ///
    /// # Panics
    ///
    /// When `relation` is not a `belongs_to_many` relation of `M` to `R`.
    pub fn has_attached<R: HasFactory>(
        self,
        children: Factory<R>,
        relation: &str,
        pivot: Vec<(&'static str, Value)>,
    ) -> Self {
        let relation_info = child_relation::<M, R>(relation);
        assert!(
            relation_info.kind == RelationKind::BelongsToMany,
            "factory: `{relation}` of `{}` is not a belongs_to_many relation",
            M::TABLE
        );
        self.push_children(children, relation, relation_info, pivot)
    }

    fn push_children<R: HasFactory>(
        mut self,
        factory: Factory<R>,
        name: &str,
        relation: Relation,
        pivot: Values,
    ) -> Self {
        self.children.push(Box::new(Has {
            factory,
            name: name.to_owned(),
            relation,
            pivot,
        }));
        self
    }

    /// Takes the parents of [`for_factory`](Self::for_factory) from
    /// `models` instead of creating them, in turn, like Laravel's
    /// `recycle`. Passes on to the factories of [`has`](Self::has) and
    /// [`for_factory`](Self::for_factory), so nested models share them too.
    pub fn recycle<'r, P: Model>(mut self, models: impl IntoIterator<Item = &'r P>) -> Self {
        let rows = models.into_iter().map(Model::to_values).collect();
        self.recycled.push((TypeId::of::<P>(), rows));
        self
    }

    /// [`count`](Self::count) unsaved models.
    pub fn make(&self) -> Collection<M> {
        self.make_n(self.count)
    }

    /// One unsaved model, ignoring [`count`](Self::count).
    pub fn make_one(&self) -> M {
        self.make_n(1).into_inner().remove(0)
    }

    fn make_n(&self, count: usize) -> Collection<M> {
        let turn = RECYCLE.fetch_add(1, Ordering::Relaxed);
        (0..count)
            .map(|index| {
                let parents = (self.parents.iter())
                    .flat_map(|parent| match parent {
                        Parent::Values(values) => values.clone(),
                        Parent::Factory(factory) => factory
                            .recycled(&self.recycled, turn + index)
                            .unwrap_or_default(),
                    })
                    .collect();
                self.build(index, parents, &[])
            })
            .collect()
    }

    /// [`count`](Self::count) models, each inserted with
    /// [`Model::save`], so [`Hooks`](crate::Hooks) and observers run.
    ///
    /// # Errors
    ///
    /// As [`Model::save`], and errors of related models and callbacks.
    /// Models saved before the error stay saved; run it in a transaction
    /// for all or nothing.
    pub async fn create(&self, db: &impl Executor) -> Result<Collection<M>> {
        self.create_n(&db.erase(), self.count, &[], &[]).await
    }

    /// One saved model, ignoring [`count`](Self::count).
    ///
    /// # Errors
    ///
    /// As [`create`](Self::create).
    pub async fn create_one(&self, db: &impl Executor) -> Result<M> {
        let models = self.create_n(&db.erase(), 1, &[], &[]).await?;
        Ok(models.into_inner().remove(0))
    }

    /// `count` saved models with `forced` column values, using the
    /// `inherited` recycled models as well as this factory's own.
    async fn create_n(
        &self,
        db: &AnyExecutor<'_>,
        count: usize,
        inherited: &[Recycled],
        forced: &[(&'static str, Value)],
    ) -> Result<Collection<M>> {
        let recycled: Vec<Recycled> = self.recycled.iter().chain(inherited).cloned().collect();
        // Keys of the parent each `for_factory` created, once per call.
        let mut created: Vec<Option<Values>> = vec![None; self.parents.len()];
        let mut models = Vec::with_capacity(count);
        let turn = RECYCLE.fetch_add(1, Ordering::Relaxed);
        for index in 0..count {
            let mut parents = Values::new();
            for (parent, created) in self.parents.iter().zip(&mut created) {
                match parent {
                    Parent::Values(values) => parents.extend(values.iter().cloned()),
                    Parent::Factory(factory) => match factory.recycled(&recycled, turn + index) {
                        Some(values) => parents.extend(values),
                        None => {
                            if created.is_none() {
                                *created = Some(factory.create(db, &recycled).await?);
                            }
                            parents.extend(created.iter().flatten().cloned());
                        }
                    },
                }
            }
            let mut model = self.build(index, parents, forced);
            model.save(db).await?;
            for children in &self.children {
                children.create(db, &model, &recycled).await?;
            }
            models.push(model);
        }
        for model in &mut models {
            for callback in &self.after_creating {
                callback(model, db).await?;
            }
        }
        Ok(models.into())
    }

    fn build(&self, index: usize, parents: Values, forced: &[(&'static str, Value)]) -> M {
        let mut model = M::definition(SEQUENCE.fetch_add(1, Ordering::Relaxed));
        set(&mut model, parents);
        for state in &self.states {
            state(&mut model, index);
        }
        set(&mut model, forced.iter().cloned());
        for callback in &self.after_making {
            callback(&mut model);
        }
        model
    }
}

/// Sets `values` on `model`.
fn set<M: Model>(model: &mut M, values: impl IntoIterator<Item = (&'static str, Value)>) {
    for (column, value) in values {
        model
            .set_value(column, value)
            .unwrap_or_else(|e| panic!("factory: cannot set `{column}` of `{}`: {e}", M::TABLE));
    }
}

/// The relation `name` of `M` that [`Factory::has`] creates `R` models for.
fn child_relation<M: Model, R: Model>(name: &str) -> Relation {
    let relation = M::relation(name)
        .unwrap_or_else(|| panic!("factory: model `{}` has no relation `{name}`", M::TABLE));
    assert!(
        relation.related_table == R::TABLE,
        "factory: relation `{name}` of `{}` is not to `{}`",
        M::TABLE,
        R::TABLE
    );
    match relation.kind {
        RelationKind::BelongsTo => panic!(
            "factory: `{name}` is a belongs_to relation; use `for_` / `for_factory` on the child's factory"
        ),
        RelationKind::HasManyThrough | RelationKind::HasOneThrough => panic!(
            "factory: `{name}` is a has_many_through / has_one_through relation; create the intermediate models instead"
        ),
        _ => relation,
    }
}

/// A parent of [`Factory::for_`] or [`Factory::for_factory`].
enum Parent {
    /// Keys of an existing parent.
    Values(Values),
    Factory(Box<dyn ParentFactory>),
}

/// A [`Factory`] of a parent model, with its type erased.
trait ParentFactory: Send + Sync {
    /// The child's key values for a recycled parent, if there is one:
    /// the one at `turn`, wrapping around.
    fn recycled(&self, recycled: &[Recycled], turn: usize) -> Option<Values>;

    /// Creates a parent and returns the child's key values for it.
    fn create<'a>(
        &'a self,
        db: &'a AnyExecutor<'a>,
        recycled: &'a [Recycled],
    ) -> BoxFuture<'a, Result<Values>>;
}

struct ForFactory<P> {
    factory: Factory<P>,
    keys: ParentKeys,
}

impl<P: HasFactory> ParentFactory for ForFactory<P> {
    fn recycled(&self, recycled: &[Recycled], turn: usize) -> Option<Values> {
        let rows: Vec<&Vec<Value>> = (recycled.iter())
            .filter(|(model, _)| *model == TypeId::of::<P>())
            .flat_map(|(_, rows)| rows.iter())
            .collect();
        let row = rows.get(turn % rows.len().max(1))?;
        Some(self.keys.values::<P>(row))
    }

    fn create<'a>(
        &'a self,
        db: &'a AnyExecutor<'a>,
        recycled: &'a [Recycled],
    ) -> BoxFuture<'a, Result<Values>> {
        Box::pin(async move {
            let parents = self.factory.create_n(db, 1, recycled, &[]).await?;
            Ok(self.keys.values::<P>(&parents[0].to_values()))
        })
    }
}

/// Where a parent's keys go in the child: each `child` column takes the
/// parent's column at the same position, and a `morph_to` relation also
/// sets its type column to the parent's morph alias.
struct ParentKeys {
    child: Vec<&'static str>,
    parent: Vec<&'static str>,
    morph: Option<(&'static str, &'static str)>,
}

impl ParentKeys {
    /// # Panics
    ///
    /// When `relation` is not a `belongs_to` relation of `M` to `P` or a
    /// `morph_to` relation of `M`.
    fn new<M: Model, P: Model>(relation: &str) -> Self {
        if let Some(r) = M::relation(relation) {
            assert!(
                r.kind == RelationKind::BelongsTo && r.related_table == P::TABLE,
                "factory: `{relation}` of `{}` is not a belongs_to relation to `{}`",
                M::TABLE,
                P::TABLE
            );
            return Self {
                child: r.local.to_vec(),
                parent: r.remote.to_vec(),
                morph: None,
            };
        }
        let [kind, id] = M::morph_columns(relation).unwrap_or_else(|| {
            panic!("factory: model `{}` has no relation `{relation}`", M::TABLE)
        });
        Self {
            child: vec![id],
            parent: P::PRIMARY_KEY.to_vec(),
            morph: Some((kind, P::MORPH_ALIAS)),
        }
    }

    /// The child's values for a parent `row` in `P::COLUMNS` order.
    fn values<P: Model>(&self, row: &[Value]) -> Values {
        let mut values: Values = (self.child.iter().zip(&self.parent))
            .map(|(child, parent)| (*child, row[column_index::<P>(parent)].clone()))
            .collect();
        values.extend(self.morph.map(|(column, alias)| (column, alias.into())));
        values
    }
}

/// The related models of [`Factory::has`], with their type erased.
trait Children<M>: Send + Sync {
    /// Creates the models related to the saved `parent`.
    fn create<'a>(
        &'a self,
        db: &'a AnyExecutor<'a>,
        parent: &'a M,
        recycled: &'a [Recycled],
    ) -> BoxFuture<'a, Result<()>>;
}

struct Has<R> {
    factory: Factory<R>,
    name: String,
    relation: Relation,
    pivot: Values,
}

impl<M: Model, R: HasFactory> Children<M> for Has<R> {
    fn create<'a>(
        &'a self,
        db: &'a AnyExecutor<'a>,
        parent: &'a M,
        recycled: &'a [Recycled],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let count = self.factory.count;
            let Some(through) = self.relation.through else {
                let keys = column_values(parent, self.relation.local)?;
                let mut values: Values = self.relation.remote.iter().copied().zip(keys).collect();
                values.extend((self.relation.morph).map(|(column, alias)| (column, alias.into())));
                self.factory.create_n(db, count, recycled, &values).await?;
                return Ok(());
            };
            let related = self.factory.create_n(db, count, recycled, &[]).await?;
            let keys = (related.iter())
                .map(|model| column_values(model, through.related_key))
                .collect::<Result<_>>()?;
            let pivot = Pivot::<R>::new(parent, &self.name)?;
            pivot.insert(db, keys, self.pivot.clone()).await
        })
    }
}
