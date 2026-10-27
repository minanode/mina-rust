//! A flat, breadth-first vector with immutable shared chunks.
//!
//! A scan-state clone shares both the chunk directory and the values. Editing
//! copies the directory once and only the chunks touched. No global interning,
//! hashing, I/O, or fallible lookup is involved. Iteration preserves Vec order.

use std::{
    collections::BTreeSet,
    ops::{Index, IndexMut, Range},
    sync::Arc,
};

const CHUNK: usize = 32;

pub(super) struct SharedValues<T> {
    chunks: Arc<Vec<Arc<[T]>>>,
    len: usize,
}

impl<T> Clone for SharedValues<T> {
    fn clone(&self) -> Self {
        Self {
            chunks: self.chunks.clone(),
            len: self.len,
        }
    }
}

impl<T> From<Vec<T>> for SharedValues<T> {
    fn from(values: Vec<T>) -> Self {
        let len = values.len();
        let mut input = values.into_iter();
        let chunks = (0..len.div_ceil(CHUNK))
            .map(|_| Arc::from(input.by_ref().take(CHUNK).collect::<Vec<_>>()))
            .collect();
        Self {
            chunks: Arc::new(chunks),
            len,
        }
    }
}

impl<T> FromIterator<T> for SharedValues<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl<T> SharedValues<T> {
    pub(super) fn len(&self) -> usize {
        self.len
    }
    pub(super) fn iter(&self) -> Iter<'_, T> {
        Iter {
            values: self,
            range: 0..self.len,
        }
    }

    /// Requested allocation sizes, including Arc headers, excluding T's owned
    /// payloads. The caller shares `seen` across live scan states to count the
    /// union of the tree structure rather than charge each alias a full tree.
    pub(super) fn heap_bytes(&self, seen: &mut BTreeSet<usize>) -> usize {
        use std::mem::size_of;
        let mut bytes = 0;
        if seen.insert(Arc::as_ptr(&self.chunks) as usize) {
            bytes += 2 * size_of::<usize>()
                + size_of::<Vec<Arc<[T]>>>()
                + self.chunks.capacity() * size_of::<Arc<[T]>>();
        }
        for chunk in self.chunks.iter() {
            if seen.insert(Arc::as_ptr(chunk) as *const T as usize) {
                bytes += 2 * size_of::<usize>() + std::mem::size_of_val(chunk.as_ref());
            }
        }
        bytes
    }
}

impl<T> Index<usize> for SharedValues<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        assert!(index < self.len);
        &self.chunks[index / CHUNK][index % CHUNK]
    }
}
impl<T: Clone> IndexMut<usize> for SharedValues<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        assert!(index < self.len);
        let chunks = Arc::make_mut(&mut self.chunks);
        &mut Arc::make_mut(&mut chunks[index / CHUNK])[index % CHUNK]
    }
}

pub(super) struct Iter<'a, T> {
    values: &'a SharedValues<T>,
    range: Range<usize>,
}
impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|i| &self.values[i])
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.range.size_hint()
    }
}
impl<T> DoubleEndedIterator for Iter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.range.next_back().map(|i| &self.values[i])
    }
}
impl<T> ExactSizeIterator for Iter<'_, T> {}
impl<'a, T> IntoIterator for &'a SharedValues<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_boundaries_and_allocation_lifetimes() {
        for len in [0, 1, 2, 31, 32, 33, 63, 64, 65, 127, 255] {
            let original: SharedValues<_> = (0..len).collect();
            let mut edited = original.clone();
            for index in 0..len {
                edited[index] = usize::MAX;
                assert_eq!(original[index], index);
            }
            assert_eq!(original.iter().len(), len);
            assert_eq!(edited.iter().rev().count(), len);
            let directory = Arc::downgrade(&edited.chunks);
            let chunks: Vec<_> = edited.chunks.iter().map(Arc::downgrade).collect();
            drop(edited);
            if len > 0 {
                assert!(directory.upgrade().is_none());
                assert!(chunks.iter().all(|chunk| chunk.upgrade().is_none()));
            }
            assert_eq!(
                original.iter().copied().collect::<Vec<_>>(),
                (0..len).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn edits_detach_only_the_changed_chunk_and_keep_old_snapshots() {
        let original: SharedValues<_> = (0..CHUNK * 3 + 1).collect();
        let mut edited = original.clone();
        assert!(Arc::ptr_eq(&original.chunks, &edited.chunks));
        edited[CHUNK + 3] = usize::MAX;
        assert!(!Arc::ptr_eq(&original.chunks, &edited.chunks));
        for i in 0..original.chunks.len() {
            assert_eq!(Arc::ptr_eq(&original.chunks[i], &edited.chunks[i]), i != 1);
        }
        assert_eq!(original[CHUNK + 3], CHUNK + 3);
        assert_eq!(edited[CHUNK + 3], usize::MAX);
        assert_eq!(
            original.iter().copied().collect::<Vec<_>>(),
            (0..CHUNK * 3 + 1).collect::<Vec<_>>()
        );
        assert_eq!(
            original.iter().rev().copied().collect::<Vec<_>>(),
            (0..CHUNK * 3 + 1).rev().collect::<Vec<_>>()
        );
        let mut seen = BTreeSet::new();
        let first = original.heap_bytes(&mut seen);
        assert_eq!(original.clone().heap_bytes(&mut seen), 0);
        assert!(edited.heap_bytes(&mut seen) < first);
    }
}
