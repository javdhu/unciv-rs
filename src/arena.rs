//! Entities by id, never by pointer.
//!
//! Unciv's hardest class of bug is the dangling reference: a unit holding a
//! tile, a city holding a civ, something removed, and a whole `transients`
//! layer existing to rebuild the object graph after a load. In Rust that shape
//! does not compile at all, so the choice is forced early — and the version
//! worth having is ids with generations.
//!
//! Two containers, because ruleset data and world state have opposite needs:
//!
//! * [`Registry`] — filled once at load, never mutated, entries never removed.
//!   Ruleset objects. Ids are plain indices and name lookup happens once.
//! * [`Arena`] — units, cities, anything that comes and goes. Slots are reused,
//!   so ids carry a generation and a stale one resolves to `None` rather than
//!   to whatever now occupies the slot.
//!
//! Both iterate in index order. That matters more than it looks: a `HashMap`
//! would iterate differently between two processes, and any rule that sums or
//! picks-first over entities would then desync client from server.

use std::collections::HashMap;
use std::fmt;
use std::marker::PhantomData;
use std::num::NonZeroU32;

/// A handle to a `T`. `Copy`, 8 bytes, and `Option<Id<T>>` is also 8 bytes.
pub struct Id<T> {
    index: u32,
    generation: NonZeroU32,
    /// `fn() -> T` rather than `T`, so `Id<T>` stays `Copy + Send + Sync`
    /// whatever `T` is.
    _marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    fn new(index: u32, generation: NonZeroU32) -> Id<T> {
        Id { index, generation, _marker: PhantomData }
    }

    pub fn index(self) -> u32 {
        self.index
    }

    pub fn generation(self) -> u32 {
        self.generation.get()
    }
}

// Derived impls would demand `T: Clone` and friends, which is wrong — the id
// is plain data and does not care what it points at.
impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, o: &Self) -> bool {
        self.index == o.index && self.generation == o.generation
    }
}
impl<T> Eq for Id<T> {}
impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        (self.index, self.generation).cmp(&(o.index, o.generation))
    }
}
impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.index.hash(h);
        self.generation.hash(h);
    }
}
impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}v{}", self.index, self.generation)
    }
}

// ---------------------------------------------------------------- Registry

/// Load-once, read-forever storage keyed by the ruleset's own names.
#[derive(Debug, Clone)]
pub struct Registry<T> {
    items: Vec<T>,
    by_name: HashMap<String, Id<T>>,
    names: Vec<String>,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Registry { items: Vec::new(), by_name: HashMap::new(), names: Vec::new() }
    }
}

impl<T> Registry<T> {
    pub fn new() -> Registry<T> {
        Registry::default()
    }

    /// Returns the existing id if `name` was already registered, so a mod
    /// redefining an object replaces it instead of shadowing it.
    pub fn insert(&mut self, name: impl Into<String>, value: T) -> Id<T> {
        let name = name.into();
        const GEN: NonZeroU32 = NonZeroU32::new(1).unwrap();
        if let Some(&id) = self.by_name.get(&name) {
            self.items[id.index as usize] = value;
            return id;
        }
        let id = Id::new(self.items.len() as u32, GEN);
        self.items.push(value);
        self.names.push(name.clone());
        self.by_name.insert(name, id);
        id
    }

    pub fn id(&self, name: &str) -> Option<Id<T>> {
        self.by_name.get(name).copied()
    }

    pub fn get(&self, id: Id<T>) -> Option<&T> {
        self.items.get(id.index as usize)
    }

    /// Mutable access, for the derive pass that runs while a ruleset is still
    /// being assembled. Nothing should call this once loading is done.
    pub fn get_mut(&mut self, id: Id<T>) -> Option<&mut T> {
        self.items.get_mut(id.index as usize)
    }

    pub fn by_name(&self, name: &str) -> Option<&T> {
        self.id(name).and_then(|id| self.get(id))
    }

    pub fn name_of(&self, id: Id<T>) -> Option<&str> {
        self.names.get(id.index as usize).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Id<T>, &T)> + '_ {
        const GEN: NonZeroU32 = NonZeroU32::new(1).unwrap();
        self.items.iter().enumerate().map(|(i, v)| (Id::new(i as u32, GEN), v))
    }

    pub fn values(&self) -> impl Iterator<Item = &T> + '_ {
        self.items.iter()
    }

    pub fn ids(&self) -> impl Iterator<Item = Id<T>> + '_ {
        self.iter().map(|(id, _)| id)
    }
}

impl<T> std::ops::Index<Id<T>> for Registry<T> {
    type Output = T;
    fn index(&self, id: Id<T>) -> &T {
        self.get(id).expect("registry ids are stable and always valid")
    }
}

// ------------------------------------------------------------------- Arena

#[derive(Debug, Clone, PartialEq)]
enum Slot<T> {
    Occupied(T),
    Vacant,
}

/// Storage for things that are created and destroyed during a game.
#[derive(Debug, Clone, PartialEq)]
pub struct Arena<T> {
    slots: Vec<(NonZeroU32, Slot<T>)>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Arena { slots: Vec::new(), free: Vec::new(), len: 0 }
    }
}

impl<T> Arena<T> {
    pub fn new() -> Arena<T> {
        Arena::default()
    }

    pub fn insert(&mut self, value: T) -> Id<T> {
        const GEN: NonZeroU32 = NonZeroU32::new(1).unwrap();
        self.len += 1;
        // Reuse the lowest free slot so iteration order stays compact and
        // reproducible rather than depending on the order things died in.
        if let Some(i) = self.free.pop() {
            let slot = &mut self.slots[i as usize];
            slot.1 = Slot::Occupied(value);
            return Id::new(i, slot.0);
        }
        self.slots.push((GEN, Slot::Occupied(value)));
        Id::new(self.slots.len() as u32 - 1, GEN)
    }

    fn slot(&self, id: Id<T>) -> Option<&(NonZeroU32, Slot<T>)> {
        let s = self.slots.get(id.index as usize)?;
        (s.0 == id.generation).then_some(s)
    }

    pub fn get(&self, id: Id<T>) -> Option<&T> {
        match self.slot(id)? {
            (_, Slot::Occupied(v)) => Some(v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, id: Id<T>) -> Option<&mut T> {
        let s = self.slots.get_mut(id.index as usize)?;
        match s {
            (g, Slot::Occupied(v)) if *g == id.generation => Some(v),
            _ => None,
        }
    }

    pub fn contains(&self, id: Id<T>) -> bool {
        self.get(id).is_some()
    }

    /// Frees the slot and bumps its generation, so every existing id to it
    /// stops resolving. This is the whole point.
    pub fn remove(&mut self, id: Id<T>) -> Option<T> {
        let s = self.slots.get_mut(id.index as usize)?;
        if s.0 != id.generation {
            return None;
        }
        let taken = match std::mem::replace(&mut s.1, Slot::Vacant) {
            Slot::Occupied(v) => v,
            Slot::Vacant => return None,
        };
        s.0 = s.0.checked_add(1).unwrap_or(NonZeroU32::new(1).unwrap());
        self.free.push(id.index);
        self.free.sort_unstable_by(|a, b| b.cmp(a));
        self.len -= 1;
        Some(taken)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (Id<T>, &T)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, (g, s))| match s {
            Slot::Occupied(v) => Some((Id::new(i as u32, *g), v)),
            Slot::Vacant => None,
        })
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Id<T>, &mut T)> + '_ {
        self.slots.iter_mut().enumerate().filter_map(|(i, (g, s))| match s {
            Slot::Occupied(v) => Some((Id::new(i as u32, *g), v)),
            Slot::Vacant => None,
        })
    }

    pub fn ids(&self) -> Vec<Id<T>> {
        self.iter().map(|(id, _)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone)]
    struct Unit(&'static str);

    #[test]
    fn a_stale_id_does_not_resolve_to_the_new_occupant() {
        let mut a = Arena::new();
        let warrior = a.insert(Unit("Warrior"));
        assert_eq!(a.remove(warrior), Some(Unit("Warrior")));

        let archer = a.insert(Unit("Archer"));
        assert_eq!(archer.index(), warrior.index(), "the slot was reused");
        assert_ne!(archer, warrior, "but the id is not the same id");

        // The bug this whole module exists to prevent.
        assert_eq!(a.get(warrior), None);
        assert_eq!(a.get(archer), Some(&Unit("Archer")));
        assert!(!a.contains(warrior));
    }

    #[test]
    fn removing_twice_is_harmless() {
        let mut a = Arena::new();
        let id = a.insert(Unit("Scout"));
        assert!(a.remove(id).is_some());
        assert!(a.remove(id).is_none());
        assert_eq!(a.len(), 0);
    }

    #[test]
    fn iteration_order_is_by_index_not_by_history() {
        let mut a = Arena::new();
        let ids: Vec<_> = ["a", "b", "c", "d"].map(|n| a.insert(Unit(n))).into();
        a.remove(ids[1]);
        a.remove(ids[2]);
        a.insert(Unit("e"));
        a.insert(Unit("f"));
        // Slots 1 and 2 were refilled, so order follows position, not insertion.
        let seen: Vec<&str> = a.iter().map(|(_, u)| u.0).collect();
        assert_eq!(seen, ["a", "e", "f", "d"]);
    }

    #[test]
    fn ids_are_eight_bytes_and_options_are_free() {
        assert_eq!(std::mem::size_of::<Id<Unit>>(), 8);
        assert_eq!(std::mem::size_of::<Option<Id<Unit>>>(), 8);
    }

    #[test]
    fn registry_resolves_names_once() {
        let mut r = Registry::new();
        let lib = r.insert("Library", Unit("Library"));
        assert_eq!(r.id("Library"), Some(lib));
        assert_eq!(r.name_of(lib), Some("Library"));
        assert_eq!(r.by_name("Library"), Some(&Unit("Library")));
        assert_eq!(r.id("Nope"), None);
    }

    #[test]
    fn redefining_a_name_replaces_rather_than_shadows() {
        let mut r = Registry::new();
        let first = r.insert("Library", Unit("vanilla"));
        let second = r.insert("Library", Unit("modded"));
        assert_eq!(first, second, "a mod overriding an object keeps its id");
        assert_eq!(r.len(), 1);
        assert_eq!(r[first], Unit("modded"));
    }
}
