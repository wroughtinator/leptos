//! Ordered sets of reactive graph edges. Small sets are inline; larger sets
//! promote to a hash index while preserving subscription order.

use super::small_set::SmallSet;
use super::{AnySource, AnySubscriber, Source};
use std::mem;

#[derive(Default, Clone, Debug)]
pub(crate) struct SourceSet(SmallSet<AnySource>);

impl SourceSet {
    pub fn new() -> Self {
        Self(Default::default())
    }

    pub fn insert(&mut self, source: AnySource) {
        self.0.insert(source);
    }

    pub fn remove(&mut self, source: &AnySource) {
        self.0.shift_remove(source);
    }

    pub fn take(&mut self) -> SmallSet<AnySource> {
        mem::take(&mut self.0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn clear_sources(&mut self, subscriber: &AnySubscriber) {
        for source in self.take() {
            source.remove_subscriber(subscriber);
        }
    }
}

impl IntoIterator for SourceSet {
    type Item = AnySource;
    type IntoIter = <SmallSet<AnySource> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a SourceSet {
    type Item = &'a AnySource;
    type IntoIter = <&'a SmallSet<AnySource> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.0).into_iter()
    }
}
#[derive(Debug, Default, Clone)]
pub(crate) struct SubscriberSet(SmallSet<AnySubscriber>);

impl SubscriberSet {
    pub fn new() -> Self {
        Self(SmallSet::default())
    }

    pub fn subscribe(&mut self, subscriber: AnySubscriber) {
        self.0.insert(subscriber);
    }

    pub fn unsubscribe(&mut self, subscriber: &AnySubscriber) {
        // note: do not use `.swap_remove()` here.
        // using `.remove()` is slower because it shifts other items
        // but it maintains the order of the subscribers, which is important
        // to correctness when you're using this to drive something like a UI,
        // which can have nested effects, where the inner one assumes the outer
        // has already run (for example, an outer effect that checks .is_some(),
        // and an inner effect that unwraps)
        self.0.shift_remove(subscriber);
    }

    pub fn take(&mut self) -> SmallSet<AnySubscriber> {
        mem::take(&mut self.0)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl IntoIterator for SubscriberSet {
    type Item = AnySubscriber;
    type IntoIter = <SmallSet<AnySubscriber> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a SubscriberSet {
    type Item = &'a AnySubscriber;
    type IntoIter = <&'a SmallSet<AnySubscriber> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        (&self.0).into_iter()
    }
}
