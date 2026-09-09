//! A map that records which keys a block touched.
//!
//! The state root is incremental: a block rehashes the rows it wrote and
//! their paths, and leaves every other leaf alone (`crate::root::RootCache`).
//! That needs an answer to "which keys moved", and the answer has to be
//! sound in one direction only — over-reporting costs one leaf hash,
//! under-reporting publishes a root that does not commit to the ledger.
//!
//! **So change tracking is a property of the container, not of the call
//! sites.** A discipline of "remember to record what you wrote" is a
//! discipline every future transition has to keep, and the day one forgets
//! is the day a validator certifies a root for a state it does not hold.
//! Here the only mutable paths are this type's own methods: the point
//! operations record their key, the whole-map operations mark everything,
//! and [`Journal::raw_mut`] — the one door that records nothing — has three
//! callers, all of them in `state.rs`, each sitting in the same function as
//! the [`Journal::touch`] calls covering exactly the keys the callee writes.
//! `DerefMut` is what closes the set: there is no `&mut` path to the map
//! that does not pass through a method here.
//!
//! The precision is free where it matters, because the per-block paths are
//! all point operations and every whole-map mutation in the tree is either
//! at the epoch boundary — which rebuilds the tree anyway, since the leaf
//! salt binds the epoch — or governed.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Where `Keys` collapses to `All`.
///
/// A patch of that many leaves is not cheaper than rebuilding the section,
/// and the cap is also what bounds memory in a state nobody drains — a
/// search run with the audit off never calls `take_dirty`, and without a
/// ceiling the set would grow with the run rather than with the block.
pub const JOURNAL_KEYS_CAP: usize = 1 << 16;

/// What a journal knows about the keys written since it was last drained.
///
/// `Keys` is exact-or-larger, never smaller: a key that appears did not
/// necessarily change, and a key that changed always appears.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dirty<K> {
    Clean,
    Keys(BTreeSet<K>),
    All,
}

impl<K> Dirty<K> {
    /// The safe reading of an unknown history — what a journal deserialised
    /// from a snapshot reports, since the cache that meets it is cold.
    pub fn all() -> Self {
        Dirty::All
    }

    pub fn is_clean(&self) -> bool {
        matches!(self, Dirty::Clean)
    }
}

/// A `BTreeMap` that records the keys of every mutation it can attribute,
/// and marks itself wholly dirty for every mutation it cannot.
#[derive(Debug)]
pub struct Journal<K: Ord, V> {
    map: BTreeMap<K, V>,
    dirty: Dirty<K>,
}

impl<K: Ord, V> Default for Journal<K, V> {
    fn default() -> Self {
        Journal { map: BTreeMap::new(), dirty: Dirty::Clean }
    }
}

/// **The dirty set is not cloned as `Clean`.** A clone is a second state
/// that a cache may later be refreshed against — the working copy a block
/// applies to, a control aged beside a treatment — and it inherits exactly
/// the pending writes the original had.
impl<K: Ord + Clone, V: Clone> Clone for Journal<K, V> {
    fn clone(&self) -> Self {
        Journal { map: self.map.clone(), dirty: self.dirty.clone() }
    }
}

/// Two journals are equal when their MAPS are: the dirty set is bookkeeping
/// about how a state was reached, and two ledgers that hold the same rows
/// are the same ledger however each got there.
impl<K: Ord, V: PartialEq> PartialEq for Journal<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl<K: Ord, V: Eq> Eq for Journal<K, V> {}

/// Serialised as the bare map — the journal is not part of the consensus
/// encoding, and a snapshot that carried it would commit to bookkeeping.
impl<K: Ord + Serialize, V: Serialize> Serialize for Journal<K, V> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.map.serialize(s)
    }
}

/// Deserialised as `All`, which is the only honest reading: a state read
/// back from a snapshot arrives with no history, and the cache that meets
/// it is cold.
impl<'de, K: Ord + Deserialize<'de>, V: Deserialize<'de>> Deserialize<'de> for Journal<K, V> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Journal { map: BTreeMap::deserialize(d)?, dirty: Dirty::All })
    }
}

impl<K: Ord, V> From<BTreeMap<K, V>> for Journal<K, V> {
    fn from(map: BTreeMap<K, V>) -> Self {
        Journal { map, dirty: Dirty::All }
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for Journal<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Journal { map: iter.into_iter().collect(), dirty: Dirty::All }
    }
}

/// Every READ is the map's own, unchanged: `get`, `len`, `iter`, `range`,
/// indexing, and the coercion that lets `&state.edges` stand where a
/// `&flow::Edges` is expected.
impl<K: Ord, V> Deref for Journal<K, V> {
    type Target = BTreeMap<K, V>;
    fn deref(&self) -> &BTreeMap<K, V> {
        &self.map
    }
}

/// **The guarantee.** Any `&mut` to the map that is not one of the methods
/// below arrives through here, and marks everything — so a mutation this
/// type cannot attribute can only ever cost a rebuild, never a stale leaf.
impl<K: Ord, V> DerefMut for Journal<K, V> {
    fn deref_mut(&mut self) -> &mut BTreeMap<K, V> {
        self.dirty = Dirty::All;
        &mut self.map
    }
}

impl<'a, K: Ord, V> IntoIterator for &'a Journal<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::collections::btree_map::Iter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

impl<K: Ord, V> IntoIterator for Journal<K, V> {
    type Item = (K, V);
    type IntoIter = std::collections::btree_map::IntoIter<K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.map.into_iter()
    }
}

impl<K: Ord + Clone, V> Journal<K, V> {
    /// Record that `key`'s row may have moved. The only thing a
    /// [`Journal::raw_mut`] caller has to get right, and the probe that
    /// holds the cache to the definition after every transition is what
    /// checks that they did.
    pub fn touch(&mut self, key: &K) {
        self.mark(key.clone());
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.mark(key.clone());
        self.map.insert(key, value)
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        self.mark(key.clone());
        self.map.remove(key)
    }

    /// Marks whether or not the key is there: a caller that asked for a
    /// mutable reference is assumed to have used it, and a mark for a key
    /// the map does not hold costs the cache nothing at all.
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        self.mark(key.clone());
        self.map.get_mut(key)
    }

    /// Marks unconditionally, for the reason `get_mut` does: the entry may
    /// be inserted, replaced or merely read, and only the first two need the
    /// mark. Over-marking is one leaf recomputed.
    pub fn entry(&mut self, key: K) -> std::collections::btree_map::Entry<'_, K, V> {
        self.mark(key.clone());
        self.map.entry(key)
    }

    fn mark(&mut self, key: K) {
        match &mut self.dirty {
            Dirty::All => {}
            Dirty::Clean => {
                self.dirty = Dirty::Keys(BTreeSet::from([key]));
            }
            Dirty::Keys(keys) => {
                keys.insert(key);
                if keys.len() > JOURNAL_KEYS_CAP {
                    self.dirty = Dirty::All;
                }
            }
        }
    }
}

impl<K: Ord, V> Journal<K, V> {
    /// The map beneath, with NOTHING recorded.
    ///
    /// Three callers, all in `state.rs`, all wrapping a kernel function that
    /// writes a set of keys the caller can name: `flow::stake`,
    /// `flow::reserve` and `flow::release`. Each pairs this with the
    /// [`Journal::touch`] calls covering exactly those keys. Anywhere else,
    /// take `&mut` and let `DerefMut` mark the map.
    pub fn raw_mut(&mut self) -> &mut BTreeMap<K, V> {
        &mut self.map
    }

    /// Iterate every value mutably — attributable to no key, so everything
    /// is marked.
    pub fn values_mut(&mut self) -> std::collections::btree_map::ValuesMut<'_, K, V> {
        self.dirty = Dirty::All;
        self.map.values_mut()
    }

    pub fn iter_mut(&mut self) -> std::collections::btree_map::IterMut<'_, K, V> {
        self.dirty = Dirty::All;
        self.map.iter_mut()
    }

    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, f: F) {
        self.dirty = Dirty::All;
        self.map.retain(f);
    }

    pub fn clear(&mut self) {
        self.dirty = Dirty::All;
        self.map.clear();
    }

    /// Drain the record, leaving the journal clean. What a cache refresh
    /// calls, once per block per journal.
    pub fn take_dirty(&mut self) -> Dirty<K> {
        std::mem::replace(&mut self.dirty, Dirty::Clean)
    }

    /// Read the record without draining it.
    pub fn dirty(&self) -> &Dirty<K> {
        &self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(d: &Dirty<u64>) -> Vec<u64> {
        match d {
            Dirty::Keys(k) => k.iter().copied().collect(),
            other => panic!("expected Keys, got {other:?}"),
        }
    }

    #[test]
    fn a_fresh_journal_is_clean_and_a_point_write_names_its_key() {
        let mut j: Journal<u64, u64> = Journal::default();
        assert!(j.dirty().is_clean());
        j.insert(7, 1);
        assert_eq!(keys(j.dirty()), vec![7]);
        j.get_mut(&9);
        j.remove(&3);
        j.entry(4).or_insert(0);
        j.touch(&11);
        assert_eq!(keys(j.dirty()), vec![3, 4, 7, 9, 11]);
        assert!(j.take_dirty() != Dirty::Clean);
        assert!(j.dirty().is_clean(), "draining leaves it clean");
    }

    /// **The soundness argument, as a probe.** Every whole-map path marks
    /// everything, and `DerefMut` is what makes that a closed set: a caller
    /// reaching for `&mut` gets `All` whatever it then does.
    ///
    /// Mutation that bites: return `&mut self.map` from `deref_mut` without
    /// setting the flag. The `&mut` case below stays `Clean` while the map
    /// underneath has changed, which is a stale leaf in the state root.
    #[test]
    fn every_unattributable_mutation_marks_the_whole_map() {
        type Mutation = (&'static str, fn(&mut Journal<u64, u64>));
        let build = || -> Journal<u64, u64> { (0..4u64).map(|i| (i, i)).collect::<BTreeMap<_, _>>().into() };
        let cases: [Mutation; 5] = [
            ("values_mut", |j| j.values_mut().for_each(|v| *v += 1)),
            ("iter_mut", |j| j.iter_mut().for_each(|(_, v)| *v += 1)),
            ("retain", |j| j.retain(|k, _| *k > 0)),
            ("clear", |j| j.clear()),
            ("deref_mut", |j| {
                let m: &mut BTreeMap<u64, u64> = j;
                m.insert(99, 99);
            }),
        ];
        for (what, mutate) in cases {
            let mut j = build();
            let _ = j.take_dirty();
            mutate(&mut j);
            assert_eq!(*j.dirty(), Dirty::All, "{what} must mark the whole map");
        }
    }

    /// A patch of more leaves than a rebuild is not a patch. The collapse
    /// also bounds memory in a state nobody drains.
    #[test]
    fn a_key_set_past_the_cap_collapses_to_all() {
        let mut j: Journal<u64, u64> = Journal::default();
        for i in 0..=JOURNAL_KEYS_CAP as u64 {
            j.touch(&i);
        }
        assert_eq!(*j.dirty(), Dirty::All);
    }

    /// A state off a snapshot has no history, and the only honest reading of
    /// no history is that everything moved.
    #[test]
    fn a_journal_off_the_wire_is_wholly_dirty_and_carries_no_bookkeeping() {
        let mut j: Journal<u64, u64> = (0..3u64).map(|i| (i, i * 2)).collect::<BTreeMap<_, _>>().into();
        j.touch(&1);
        let bytes = crate::codec::encode(&j).expect("encode");
        // The bare map encodes to the same bytes: the journal is not part of
        // the consensus encoding.
        assert_eq!(bytes, crate::codec::encode(&*j).expect("encode the map"));
        let back: Journal<u64, u64> = crate::codec::decode(&bytes).expect("decode");
        assert_eq!(back, j, "equality is the map's");
        assert_eq!(*back.dirty(), Dirty::All);
    }

    /// A clone inherits the pending writes rather than starting clean: the
    /// working copy a block applies to is a second state a cache may be
    /// refreshed against.
    #[test]
    fn a_clone_inherits_the_pending_writes() {
        let mut j: Journal<u64, u64> = Journal::default();
        j.insert(5, 1);
        assert_eq!(*j.clone().dirty(), *j.dirty());
    }
}
