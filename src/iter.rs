use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::mem::MaybeUninit;

use crate::list::List;
use crate::storage::{Layout, Storage};

/// `&List` 的迭代器，对齐 `std::collections::linked_list::Iter`。
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

/// `&mut List` 的迭代器，对齐 `std::collections::linked_list::IterMut`。
///
/// 内部用裸地址（由 [`Storage::layout`] 提供）实现，这样才能在
/// `next()` 里交出 `&'a mut T` 而不与迭代器自身状态冲突。安全性由
/// `iter_mut(&mut self)` 的独占借用保证：迭代期间容器不会移动或重分配。
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

    /// 元素 `slot` 的 `data` 槽地址（基址是字节指针，所以这里只需一次 cast）。
    unsafe fn data(&self, slot: usize) -> *mut MaybeUninit<T> {
        unsafe {
            self.layout
                .data
                .add(slot * self.layout.data_stride + self.layout.data_offset)
                .cast::<MaybeUninit<T>>()
        }
    }

    /// 按链接宽度读一个下标并加宽。
    ///
    /// **不能**直接 `cast::<usize>()` 解引用：链接可能是 4 字节（`Soa<T, u32>`），
    /// 地址只有 4 字节对齐 ⇒ 未对齐读是 UB（miri 会报）。宽度在迭代器生命周期内不变，
    /// 所以 `match` 的判别式是循环不变量（LLVM 会把它提出循环，分支也可预测）。
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

    /// 只对 **live** 槽位调用（`Layout` 读的是裸值：空闲槽的 `prev` 是空闲标记位）。
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

// 与 std 的 `IterMut` 一致：独占访问，可安全地跨线程传递。
unsafe impl<T: Send, S: Storage<T>> Send for IterMut<'_, T, S> {}

unsafe impl<T: Sync, S: Storage<T>> Sync for IterMut<'_, T, S> {}

/// 所有权迭代器，对齐 `std::collections::linked_list::IntoIter`。
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
