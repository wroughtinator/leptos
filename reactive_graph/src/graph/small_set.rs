//! Ordered graph edges: keep the common zero-to-two case inline, and promote
//! larger graphs to an indexed hash set. Removal must preserve insertion order.
use indexmap::IndexSet;
use rustc_hash::FxHasher;
use std::hash::{BuildHasherDefault, Hash};

#[derive(Clone, Debug)]
pub(crate) enum SmallSet<T> {
    Inline([Option<T>; 2]),
    Indexed(IndexSet<T, BuildHasherDefault<FxHasher>>),
}

impl<T> Default for SmallSet<T> {
    fn default() -> Self {
        Self::Inline([None, None])
    }
}

impl<T: Eq + Hash> SmallSet<T> {
    pub fn insert(&mut self, value: T) {
        match self {
            Self::Inline(values) => {
                if values.iter().flatten().any(|entry| entry == &value) {
                    return;
                }
                if let Some(empty) =
                    values.iter_mut().find(|entry| entry.is_none())
                {
                    *empty = Some(value);
                } else {
                    let mut indexed = IndexSet::with_capacity_and_hasher(
                        4,
                        Default::default(),
                    );
                    indexed.extend(values.iter_mut().filter_map(Option::take));
                    indexed.insert(value);
                    *self = Self::Indexed(indexed);
                }
            }
            Self::Indexed(values) => {
                values.insert(value);
            }
        }
    }

    pub fn shift_remove(&mut self, value: &T) {
        match self {
            Self::Inline(values) => {
                if values[0].as_ref() == Some(value) {
                    values[0] = values[1].take();
                } else if values[1].as_ref() == Some(value) {
                    values[1] = None;
                }
            }
            Self::Indexed(values) => {
                values.shift_remove(value);
            }
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Inline(values) => values.iter().flatten().count(),
            Self::Indexed(values) => values.len(),
        }
    }
}

pub(crate) enum IntoIter<T> {
    Inline(std::iter::Flatten<std::array::IntoIter<Option<T>, 2>>),
    Indexed(indexmap::set::IntoIter<T>),
}
impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        match self {
            Self::Inline(i) => i.next(),
            Self::Indexed(i) => i.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Inline(i) => i.size_hint(),
            Self::Indexed(i) => i.size_hint(),
        }
    }
}
impl<T> IntoIterator for SmallSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Inline(v) => IntoIter::Inline(v.into_iter().flatten()),
            Self::Indexed(v) => IntoIter::Indexed(v.into_iter()),
        }
    }
}

pub(crate) enum Iter<'a, T> {
    Inline(std::iter::Flatten<std::slice::Iter<'a, Option<T>>>),
    Indexed(indexmap::set::Iter<'a, T>),
}
impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Inline(i) => i.next(),
            Self::Indexed(i) => i.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Inline(i) => i.size_hint(),
            Self::Indexed(i) => i.size_hint(),
        }
    }
}
impl<'a, T> IntoIterator for &'a SmallSet<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        match self {
            SmallSet::Inline(v) => Iter::Inline(v.iter().flatten()),
            SmallSet::Indexed(v) => Iter::Indexed(v.iter()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ordered_set_matches_indexset_across_promotion_removal_and_clone() {
        let mut small = SmallSet::default();
        let mut reference = IndexSet::new();
        for i in (0..100).chain((0..100).rev()) {
            small.insert(i);
            reference.insert(i);
            if i % 3 == 0 {
                small.shift_remove(&(i / 2));
                reference.shift_remove(&(i / 2));
            }
            assert_eq!(small.len(), reference.len());
            assert_eq!(
                (&small).into_iter().copied().collect::<Vec<_>>(),
                reference.iter().copied().collect::<Vec<_>>()
            );
            assert_eq!(
                small.clone().into_iter().collect::<Vec<_>>(),
                reference.iter().copied().collect::<Vec<_>>()
            );
        }
        let mut inline = SmallSet::default();
        inline.insert(1);
        inline.insert(2);
        inline.insert(2);
        inline.shift_remove(&1);
        inline.insert(3);
        assert!(matches!(inline, SmallSet::Inline(_)));
        assert_eq!(inline.into_iter().collect::<Vec<_>>(), [2, 3]);
    }
}
