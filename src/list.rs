use std::fmt;
use std::iter::{Extend, FromIterator};
use std::marker::PhantomData;

use crate::cursor::{Cursor, CursorMut};
use crate::iter::{IntoIter, Iter, IterMut};
use crate::storage::{Aos, NIL, Packed, Soa, Storage};

/// 双向链表：`Vec` 下标 + free-list 的实现，内存布局由 `S` 决定。
///
/// 见 crate 文档里的三个别名 [`SoaList`](crate::SoaList) /
/// [`PackedList`](crate::PackedList) / [`AosList`](crate::AosList)。
pub struct List<T, S: Storage<T>> {
    pub(crate) storage: S,
    pub(crate) head: usize,
    pub(crate) tail: usize,
    pub(crate) free_head: usize,
    pub(crate) len: usize,
    marker: PhantomData<T>,
}

impl<T> List<T, Soa<T>> {
    /// 空链表（SoA 布局）。
    pub const fn new() -> Self {
        Self {
            storage: Soa::new(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T> List<T, Packed<T>> {
    /// 空链表（Packed 布局）。
    pub const fn new() -> Self {
        Self {
            storage: Packed::new(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T> List<T, Aos<T>> {
    /// 空链表（AoS 布局）。
    pub const fn new() -> Self {
        Self {
            storage: Aos::new(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T, S: Storage<T> + Default> List<T, S> {
    /// **本库扩展**：预留 `capacity` 个槽位的容量后创建空链表。
    ///
    /// 槽位在**连续内存**里，所以预留能避免构造期的反复 realloc
    /// （对 SoA 布局尤其明显：一次预留省掉三个数组的 3×扩容）。
    pub fn with_capacity(capacity: usize) -> Self {
        let mut list = Self::default();

        list.reserve(capacity);

        list
    }
}

impl<T, S: Storage<T>> List<T, S> {
    // --------------------------------------------------------
    // 基本查询
    // --------------------------------------------------------

    /// 元素个数。
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// **本库扩展**（std 的链表没有容量概念）：预留 `additional` 个槽位的容量。
    pub fn reserve(&mut self, additional: usize) {
        self.storage.reserve(additional);
    }

    // --------------------------------------------------------
    // 内部：节点操作
    // --------------------------------------------------------

    #[inline]
    fn move_forward(&self, index: usize, steps: usize) -> usize {
        let mut current = index;

        for _ in 0..steps {
            let next = self.storage.next(current);

            if next == NIL {
                return NIL;
            }

            current = next;
        }

        current
    }

    #[inline]
    fn move_backward(&self, index: usize, steps: usize) -> usize {
        let mut current = index;

        for _ in 0..steps {
            let prev = self.storage.prev(current);

            if prev == NIL {
                return NIL;
            }

            current = prev;
        }

        current
    }

    /// 第 `pos` 个 live 节点的下标；`pos` 必须 `< len`。
    /// 从**步数更少**的一端出发：forward 走 `pos` 步，backward 走 `len-1-pos` 步。
    #[inline]
    pub(crate) fn index_at(&self, pos: usize) -> usize {
        debug_assert!(pos < self.len);

        if pos < self.len.div_ceil(2) {
            self.move_forward(self.head, pos)
        } else {
            self.move_backward(self.tail, self.len - 1 - pos)
        }
    }

    #[inline]
    pub(crate) fn alloc_node(&mut self, value: T) -> usize {
        self.len += 1;

        let index = if self.free_head != NIL {
            let free_index = self.free_head;

            self.free_head = self.storage.next(free_index);
            free_index
        } else {
            self.storage.grow()
        };

        self.storage.data_mut(index).write(value);

        index
    }

    #[inline]
    pub(crate) fn free_node(&mut self, index: usize) -> T {
        self.len -= 1;

        let free_head = self.free_head;
        let value = unsafe { self.storage.data_mut(index).assume_init_read() };

        self.storage.set_next(index, free_head);
        self.free_head = index;

        value
    }

    #[inline]
    pub(crate) fn insert_link(&mut self, pos: usize, prev: usize, next: usize) {
        self.storage.set_prev(pos, prev);
        self.storage.set_next(pos, next);

        if prev != NIL {
            self.storage.set_next(prev, pos);
        } else {
            self.head = pos;
        }

        if next != NIL {
            self.storage.set_prev(next, pos);
        } else {
            self.tail = pos;
        }
    }

    #[inline]
    pub(crate) fn remove_link(&mut self, prev: usize, next: usize) {
        if prev != NIL {
            self.storage.set_next(prev, next);
        } else {
            self.head = next;
        }

        if next != NIL {
            self.storage.set_prev(next, prev);
        } else {
            self.tail = prev;
        }
    }

    // --------------------------------------------------------
    // 端操作
    // --------------------------------------------------------

    #[inline]
    fn push_front_index(&mut self, value: T) -> usize {
        let index = self.alloc_node(value);

        self.insert_link(index, NIL, self.head);

        index
    }

    #[inline]
    fn push_back_index(&mut self, value: T) -> usize {
        let index = self.alloc_node(value);

        self.insert_link(index, self.tail, NIL);

        index
    }

    /// 对齐 `LinkedList::push_front`。
    #[inline]
    pub fn push_front(&mut self, value: T) {
        let _ = self.push_front_index(value);
    }

    /// 对齐 `LinkedList::push_back`。
    #[inline]
    pub fn push_back(&mut self, value: T) {
        let _ = self.push_back_index(value);
    }

    /// 对齐 `LinkedList::push_front_mut`：插入并返回新元素的引用。
    #[inline]
    pub fn push_front_mut(&mut self, value: T) -> &mut T {
        let index = self.push_front_index(value);

        unsafe { self.storage.data_mut(index).assume_init_mut() }
    }

    /// 对齐 `LinkedList::push_back_mut`：插入并返回新元素的引用。
    #[inline]
    pub fn push_back_mut(&mut self, value: T) -> &mut T {
        let index = self.push_back_index(value);

        unsafe { self.storage.data_mut(index).assume_init_mut() }
    }

    /// 对齐 `LinkedList::pop_front`。
    #[inline]
    pub fn pop_front(&mut self) -> Option<T> {
        if self.head == NIL {
            return None;
        }

        let index = self.head;

        self.remove_link(NIL, self.storage.next(index));

        Some(self.free_node(index))
    }

    /// 对齐 `LinkedList::pop_back`。
    #[inline]
    pub fn pop_back(&mut self) -> Option<T> {
        if self.tail == NIL {
            return None;
        }

        let index = self.tail;

        self.remove_link(self.storage.prev(index), NIL);

        Some(self.free_node(index))
    }

    // --------------------------------------------------------
    // 端点访问
    // --------------------------------------------------------

    /// 对齐 `LinkedList::front`。
    #[inline]
    pub fn front(&self) -> Option<&T> {
        if self.head == NIL {
            return None;
        }

        Some(unsafe { self.storage.data(self.head).assume_init_ref() })
    }

    /// 对齐 `LinkedList::back`。
    #[inline]
    pub fn back(&self) -> Option<&T> {
        if self.tail == NIL {
            return None;
        }

        Some(unsafe { self.storage.data(self.tail).assume_init_ref() })
    }

    /// 对齐 `LinkedList::front_mut`。
    #[inline]
    pub fn front_mut(&mut self) -> Option<&mut T> {
        if self.head == NIL {
            return None;
        }

        Some(unsafe { self.storage.data_mut(self.head).assume_init_mut() })
    }

    /// 对齐 `LinkedList::back_mut`。
    #[inline]
    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.tail == NIL {
            return None;
        }

        Some(unsafe { self.storage.data_mut(self.tail).assume_init_mut() })
    }

    // --------------------------------------------------------
    // 游标
    // --------------------------------------------------------

    /// 对齐 `LinkedList::cursor_front`。
    #[inline]
    pub fn cursor_front(&self) -> Cursor<'_, T, S> {
        Cursor::at(self, self.head, 0)
    }

    /// 对齐 `LinkedList::cursor_front_mut`。
    #[inline]
    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T, S> {
        CursorMut::at(self, self.head, 0)
    }

    /// 对齐 `LinkedList::cursor_back`。
    #[inline]
    pub fn cursor_back(&self) -> Cursor<'_, T, S> {
        Cursor::at(self, self.tail, self.len.saturating_sub(1))
    }

    /// 对齐 `LinkedList::cursor_back_mut`。
    #[inline]
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, T, S> {
        CursorMut::at(self, self.tail, self.len.saturating_sub(1))
    }

    /// **本库扩展**（std 没有随机访问）：定位到第 `pos` 个元素。
    /// 从较近的一端出发，O(min(pos, len - 1 - pos))。
    #[inline]
    pub fn at(&mut self, pos: usize) -> Option<CursorMut<'_, T, S>> {
        if pos >= self.len {
            return None;
        }

        let index = self.index_at(pos);

        Some(CursorMut::at(self, index, pos))
    }

    // --------------------------------------------------------
    // 迭代
    // --------------------------------------------------------

    /// 对齐 `LinkedList::iter`。
    #[inline]
    pub fn iter(&self) -> Iter<'_, T, S> {
        Iter::new(self)
    }

    /// 对齐 `LinkedList::iter_mut`。
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, T, S> {
        IterMut::new(self)
    }

    // --------------------------------------------------------
    // 批量操作
    // --------------------------------------------------------

    /// 对齐 `LinkedList::contains`。O(N)。
    #[inline]
    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|item| item == value)
    }

    /// 对齐 `LinkedList::retain`。O(N)。
    #[inline]
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let mut index = self.head;

        while index != NIL {
            let next = self.storage.next(index);

            let keep = f(unsafe { self.storage.data_mut(index).assume_init_mut() });

            if !keep {
                let prev = self.storage.prev(index);

                self.remove_link(prev, next);
                let _ = self.free_node(index);
            }

            index = next;
        }
    }

    /// 对齐 `LinkedList::append`：把 `other` 的元素全部移到末尾。
    /// 与 std 的 O(1) 拼接不同，这里是 O(other.len())。
    pub fn append(&mut self, other: &mut Self) {
        while let Some(value) = other.pop_front() {
            self.push_back(value);
        }
    }

    /// 对齐 `LinkedList::split_off`。`at > len` 时 panic。O(N)。
    pub fn split_off(&mut self, at: usize) -> Self
    where
        S: Default,
    {
        assert!(at <= self.len, "split_off index out of bounds");

        let len = self.len;
        let mut tail = Self::default();

        for _ in at..len {
            let value = self.pop_back().expect("split_off: inconsistent length");

            tail.push_front(value);
        }

        tail
    }

    /// 对齐 `LinkedList::remove`。`at >= len` 时 panic。O(N)。
    pub fn remove(&mut self, at: usize) -> T {
        assert!(at < self.len, "remove index out of bounds");

        let index = self.index_at(at);
        let prev = self.storage.prev(index);
        let next = self.storage.next(index);

        self.remove_link(prev, next);

        self.free_node(index)
    }

    /// 对齐 `LinkedList::clear`。保留已分配的容量与槽位。
    ///
    /// 单趟完成：逐个析构 live 元素，并把槽位就地挂回 free list。
    #[inline]
    pub fn clear(&mut self) {
        let mut index = self.head;

        while index != NIL {
            let next = self.storage.next(index);

            unsafe { self.storage.data_mut(index).assume_init_drop() };

            self.storage.set_next(index, self.free_head);
            self.free_head = index;

            index = next;
        }

        self.head = NIL;
        self.tail = NIL;
        self.len = 0;
    }
}

// ============================================================
// std 风格 trait 实现
// ============================================================

impl<T, S: Storage<T> + Default> Default for List<T, S> {
    fn default() -> Self {
        Self {
            storage: S::default(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T: Clone, S: Storage<T> + Default> Clone for List<T, S> {
    fn clone(&self) -> Self {
        let mut out = Self::default();

        for value in self {
            out.push_back(Clone::clone(value));
        }

        out
    }
}

impl<T: fmt::Debug, S: Storage<T>> fmt::Debug for List<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self).finish()
    }
}

impl<T: PartialEq, S: Storage<T>> PartialEq for List<T, S> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| *a == *b)
    }
}

impl<T: Eq, S: Storage<T>> Eq for List<T, S> {}

impl<T, S: Storage<T> + Default> FromIterator<T> for List<T, S> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut out = Self::default();

        out.extend(iter);

        out
    }
}

impl<T, S: Storage<T>> Extend<T> for List<T, S> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.push_back(value);
        }
    }
}

impl<T, S: Storage<T>> IntoIterator for List<T, S> {
    type Item = T;
    type IntoIter = IntoIter<T, S>;

    fn into_iter(self) -> IntoIter<T, S> {
        IntoIter::new(self)
    }
}

impl<'a, T, S: Storage<T>> IntoIterator for &'a List<T, S> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T, S>;

    fn into_iter(self) -> Iter<'a, T, S> {
        self.iter()
    }
}

impl<'a, T, S: Storage<T>> IntoIterator for &'a mut List<T, S> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T, S>;

    fn into_iter(self) -> IterMut<'a, T, S> {
        self.iter_mut()
    }
}

impl<T, S: Storage<T>> Drop for List<T, S> {
    fn drop(&mut self) {
        self.clear();
    }
}
