use core::iter::FusedIterator;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::list::List;
use crate::storage::{Layout, Storage};

/// Iterator over `&List`, matching `std::collections::linked_list::Iter`.
pub struct Iter<'a, T, S: Storage<T>> {
    list: &'a List<T, S>,
    front: usize,
    back: usize,
    remaining: usize,
}

impl<'a, T, S: Storage<T>> Iter<'a, T, S> {
    pub(crate) fn new(list: &'a List<T, S>) -> Self {
        Self {
            list,
            front: list.head,
            back: list.tail,
            remaining: list.len,
        }
    }
}

impl<'a, T, S: Storage<T>> Iterator for Iter<'a, T, S> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.remaining == 0 {
            return None;
        }

        let node = self.front;

        self.front = self.list.storage.next(node);
        self.remaining -= 1;

        Some(unsafe { self.list.storage.data(node).assume_init_ref() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T, S: Storage<T>> DoubleEndedIterator for Iter<'_, T, S> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let node = self.back;

        self.back = self.list.storage.prev(node);
        self.remaining -= 1;

        Some(unsafe { self.list.storage.data(node).assume_init_ref() })
    }
}

impl<T, S: Storage<T>> ExactSizeIterator for Iter<'_, T, S> {}

impl<T, S: Storage<T>> FusedIterator for Iter<'_, T, S> {}

impl<T, S: Storage<T>> Clone for Iter<'_, T, S> {
    fn clone(&self) -> Self {
        Self {
            list: self.list,
            front: self.front,
            back: self.back,
            remaining: self.remaining,
        }
    }
}

/// Iterator over `&mut List`, matching `std::collections::linked_list::IterMut`.
///
/// Implemented with raw addresses (provided by [`Storage::layout`]) so that
/// `next()` can hand out `&'a mut T` without conflicting with the iterator's own
/// state. Safety comes from the exclusive borrow in `iter_mut(&mut self)`: the
/// container cannot move or reallocate during iteration.
pub struct IterMut<'a, T, S: Storage<T>> {
    layout: Layout,
    front: usize,
    back: usize,
    remaining: usize,
    marker: PhantomData<(&'a mut T, S)>,
}

impl<'a, T, S: Storage<T>> IterMut<'a, T, S> {
    pub(crate) fn new(list: &'a mut List<T, S>) -> Self {
        let layout = list.storage.layout();

        Self {
            layout,
            front: list.head,
            back: list.tail,
            remaining: list.len,
            marker: PhantomData,
        }
    }

    /// Address of the `data` slot of element `slot` (the base is a byte pointer,
    /// so a single cast suffices here).
    unsafe fn data(&self, slot: usize) -> *mut MaybeUninit<T> {
        unsafe {
            self.layout
                .data
                .add(slot * self.layout.data_stride + self.layout.data_offset)
                .cast::<MaybeUninit<T>>()
        }
    }

    /// Read an index at the link width and widen it.
    ///
    /// We **must not** `cast::<usize>()` and dereference directly: a link may be
    /// 4 bytes (`Split<T, u32>`) and the address is only 4-byte aligned, so an
    /// unaligned read is UB (miri flags it). The width is constant for the
    /// iterator's lifetime, so the `match` discriminant is a loop invariant
    /// (LLVM hoists it out of the loop; the branch is predictable).
    #[inline(always)]
    unsafe fn read_ix(ptr: *const u8, width: usize) -> usize {
        unsafe {
            match width {
                1 => ptr.read() as usize,
                2 => ptr.cast::<u16>().read_unaligned() as usize,
                4 => ptr.cast::<u32>().read_unaligned() as usize,
                _ => ptr.cast::<u64>().read_unaligned() as usize,
            }
        }
    }

    unsafe fn next_of(&self, slot: usize) -> usize {
        unsafe {
            Self::read_ix(
                self.layout.next.add(slot * self.layout.next_stride),
                self.layout.ix_width,
            )
        }
    }

    /// Call only on **live** slots (`Layout` reads the raw value: a free slot's
    /// `prev` holds the free bit).
    unsafe fn prev_of(&self, slot: usize) -> usize {
        unsafe {
            Self::read_ix(
                self.layout.prev.add(slot * self.layout.prev_stride),
                self.layout.ix_width,
            )
        }
    }
}

impl<'a, T, S: Storage<T>> Iterator for IterMut<'a, T, S> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<&'a mut T> {
        if self.remaining == 0 {
            return None;
        }

        let node = self.front;

        self.front = unsafe { self.next_of(node) };
        self.remaining -= 1;

        Some(unsafe { (*self.data(node)).assume_init_mut() })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T, S: Storage<T>> DoubleEndedIterator for IterMut<'_, T, S> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let node = self.back;

        self.back = unsafe { self.prev_of(node) };
        self.remaining -= 1;

        Some(unsafe { (*self.data(node)).assume_init_mut() })
    }
}

impl<T, S: Storage<T>> ExactSizeIterator for IterMut<'_, T, S> {}

impl<T, S: Storage<T>> FusedIterator for IterMut<'_, T, S> {}

// Matches std's `IterMut`: exclusive access, safe to send across threads.
unsafe impl<T: Send, S: Storage<T>> Send for IterMut<'_, T, S> {}

unsafe impl<T: Sync, S: Storage<T>> Sync for IterMut<'_, T, S> {}

/// Owning iterator, matching `std::collections::linked_list::IntoIter`.
pub struct IntoIter<T, S: Storage<T>> {
    list: List<T, S>,
}

impl<T, S: Storage<T>> IntoIter<T, S> {
    pub(crate) fn new(list: List<T, S>) -> Self {
        Self { list }
    }
}

impl<T, S: Storage<T>> Iterator for IntoIter<T, S> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.list.pop_front()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.list.len();

        (len, Some(len))
    }
}

impl<T, S: Storage<T>> DoubleEndedIterator for IntoIter<T, S> {
    fn next_back(&mut self) -> Option<T> {
        self.list.pop_back()
    }
}

impl<T, S: Storage<T>> ExactSizeIterator for IntoIter<T, S> {}

impl<T, S: Storage<T>> FusedIterator for IntoIter<T, S> {}
