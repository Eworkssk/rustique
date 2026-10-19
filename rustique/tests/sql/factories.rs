use std::collections::HashSet;

use rustique::{HasFactory, Model, Rel};

#[rustique::model(table = "teams")]
#[derive(Debug, Clone, Default)]
struct Team {
    id: i64,
    name: String,
}

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    team_id: Option<i64>,
    name: String,
    #[rustique(belongs_to(Team))]
    team: Rel<Option<Team>>,
    #[rustique(has_many(Image))]
    images: Rel<Vec<Image>>,
}

#[allow(dead_code)] // only the variant's morph alias is used
#[derive(Debug, Clone, rustique::MorphTo)]
enum Imageable {
    Team(Team),
}

#[rustique::model(table = "images")]
#[derive(Debug, Clone, Default)]
struct Image {
    id: i64,
    user_id: i64,
    imageable_type: Option<String>,
    imageable_id: Option<i64>,
    #[rustique(morph_to)]
    imageable: Rel<Option<Imageable>>,
}

impl HasFactory for Team {
    fn definition(n: u64) -> Self {
        Team {
            name: format!("team {n}"),
            ..Default::default()
        }
    }
}

impl HasFactory for User {
    fn definition(n: u64) -> Self {
        User {
            name: format!("user {n}"),
            ..Default::default()
        }
    }

    fn after_making(&mut self) {
        self.name.push('#');
    }
}

impl HasFactory for Image {
    fn definition(_: u64) -> Self {
        Image::default()
    }
}

#[test]
fn make_runs_states_then_hook_then_callbacks_with_a_new_n_per_model() {
    let users = User::factory()
        .count(3)
        .state(|u| u.name.push('!'))
        .sequence(|u, i| u.id = i as i64 * 10)
        .after_making(|u| u.name.push('?'))
        .make();
    assert_eq!(users.iter().map(|u| u.id).collect::<Vec<_>>(), [0, 10, 20]);
    assert!(users.iter().all(|u| u.name.ends_with("!#?") && !u.exists()));
    let names: HashSet<_> = users.iter().map(|u| &u.name).collect();
    assert_eq!(names.len(), 3);

    let factory = User::factory().count(5);
    assert_eq!(factory.make_one().id, 0, "make_one ignores count");
    assert_ne!(factory.make_one().name, factory.make_one().name);
    assert!(User::factory().count(0).make().is_empty());
}

#[test]
fn for_sets_belongs_to_and_morph_to_keys() {
    let team = Team {
        id: 7,
        ..Default::default()
    };
    let user = User::factory().for_(&team, "team").make_one();
    assert_eq!(user.team_id, Some(7));

    // A state added later still wins over the parent's key.
    let user = User::factory()
        .for_(&team, "team")
        .state(|u| u.team_id = None)
        .make_one();
    assert_eq!(user.team_id, None);

    let image = Image::factory().for_(&team, "imageable").make_one();
    assert_eq!(image.imageable_type.as_deref(), Some("teams"));
    assert_eq!(image.imageable_id, Some(7));
}

#[test]
fn make_takes_factory_parents_from_recycled_models_in_turn() {
    let teams: Vec<Team> = (1..=2)
        .map(|id| Team {
            id,
            ..Default::default()
        })
        .collect();
    let users = User::factory()
        .count(4)
        .for_factory(Team::factory(), "team")
        .recycle(&teams)
        .make();
    let ids: HashSet<_> = users.iter().map(|u| u.team_id.unwrap()).collect();
    assert_eq!(ids, HashSet::from([1, 2]));

    // Without recycled teams, make creates none and leaves the key alone.
    let user = User::factory()
        .for_factory(Team::factory(), "team")
        .make_one();
    assert_eq!(user.team_id, None);
}

#[test]
#[should_panic(expected = "model `users` has no relation `nope`")]
fn unknown_relation_panics() {
    let _ = User::factory().for_(&Team::default(), "nope");
}

#[test]
#[should_panic(expected = "use `for_` / `for_factory` on the child's factory")]
fn has_rejects_belongs_to() {
    let _ = User::factory().has(Team::factory(), "team");
}

#[test]
#[should_panic(expected = "`images` of `users` is not a belongs_to relation to `images`")]
fn for_rejects_has_many() {
    let _ = User::factory().for_(&Image::default(), "images");
}
