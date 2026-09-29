use std::fmt;
use std::marker::PhantomData;

use crate::cursor::{Cursor, CursorMut};
use crate::iter::{IntoIter, Iter, IterMut};
use crate::storage::{Aos, NIL, Packed, Soa, Storage};

/// 双向链表：`Vec` 下标 + free-list 的实现，内存布局由 `S` 决定。
///
/// 见 crate 文档里的三个别名 [`SoaList`](crate::SoaList) /
/// [`PackedList`](crate::PackedList) / [`AosList`](crate::AosList)。
///
/// # 数组里没有哨兵值
///
/// `prev` / `next` 数组里**每个值都是合法槽位下标**。两条链的两端是"哑元"：
/// 链头节点的 `prev`、链尾节点的 `next` 没人读，但它们同样是合法下标（新槽位
/// 初始化为指向自己的自环，之后可能留着一个过期的合法下标）。
/// [`NIL`] 只作为标量参数/游标 tag 使用，**永远不写进数组**。
///
/// 这一点是 [`append`](List::append) 的地基：搬过来的整段下标可以无条件
/// `+= base`，不需要逐元素判断"这是不是哨兵"。
///
/// # 字段的空态
///
/// **四个端点字段在链为空时都归位 [`NIL`]**：`head` / `tail` 由 `len == 0`
/// 触发，`free_head` / `free_tail` 由"free 链变空"触发。于是"空不空"就是读
/// 一个字段（1 次 load + 比较），而不是算 `storage.len() - len`（2 次 load +
/// 减法）——后者每次 free/alloc 都跑，`churn` 基准上实测慢 3~6%（三个布局
/// 一致，对照组反向漂移）。归位本身只在"变空"那一次写两个字段，很便宜。
///
/// # 链表形状
///
/// 单向游走一条链必须**按个数**，不能"走到 NIL 为止"（两端的哑元不是 NIL）。
/// [`Iter`] / [`IterMut`] 本来就用 `remaining` 计数；`retain` / `clear` 也按
/// `len` 收尾。
pub struct List<T, S: Storage<T>> {
    /// 链头；空表时是 [`NIL`]。
    pub(crate) head: usize,
    /// 链尾；空表时是 [`NIL`]。
    pub(crate) tail: usize,
    /// free 链头；空闲数为 0 时无意义。
    pub(crate) free_head: usize,
    /// free 链尾；空闲数为 0 时无意义。只在"空链变非空"时更新。
    pub(crate) free_tail: usize,
    /// live 元素个数。
    pub(crate) len: usize,
    pub(crate) storage: S,
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
            free_tail: NIL,
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
            free_tail: NIL,
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
            free_tail: NIL,
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

    /// 夹在 `prev` 与 `next` 之间插入 `pos`（两者都是真实邻居）。
    #[inline]
    pub(crate) fn insert_between(&mut self, pos: usize, prev: usize, next: usize) {
        self.storage.set_next(prev, pos);
        self.storage.set_prev(next, pos);
        self.storage.set_prev(pos, prev);
        self.storage.set_next(pos, next);
    }

    /// 把 `pos` 插成链头。调用前 `pos` 已经算进 `len`（[`alloc_node`](Self::alloc_node) 干过）。
    #[inline]
    pub(crate) fn insert_front(&mut self, pos: usize) {
        if self.len == 1 {
            // 原本是空表：两条链接都是哑元（自环）
            self.storage.set_prev(pos, pos);
            self.storage.set_next(pos, pos);
            self.head = pos;
            self.tail = pos;
        } else {
            let head = self.head;

            self.storage.set_prev(head, pos);
            self.storage.set_next(pos, head);
            self.head = pos;
        }
    }

    /// 把 `pos` 插成链尾。
    #[inline]
    pub(crate) fn insert_back(&mut self, pos: usize) {
        if self.len == 1 {
            self.storage.set_prev(pos, pos);
            self.storage.set_next(pos, pos);
            self.head = pos;
            self.tail = pos;
        } else {
            let tail = self.tail;

            self.storage.set_next(tail, pos);
            self.storage.set_prev(pos, tail);
            self.tail = pos;
        }
    }

    /// 从 free 链头摘一个槽位。调用前 free 链必须非空。
    #[inline]
    fn pop_free(&mut self) -> usize {
        let index = self.free_head;
        let next = self.storage.next(index);

        if next == index {
            // 只剩它一个（自环终止）⇒ 链空了：归位
            self.free_head = NIL;
            self.free_tail = NIL;
        } else {
            self.free_head = next;
        }

        index
    }

    /// 把槽位挂回 free 链头。
    #[inline]
    fn push_free(&mut self, index: usize) {
        if self.free_head == NIL {
            // 原本是空链：它同时也是链尾，`next` 写自环（NIL 决不能进数组）
            self.free_tail = index;
            self.storage.set_next(index, index);
        } else {
            self.storage.set_next(index, self.free_head);
        }

        self.free_head = index;
    }

    #[inline]
    pub(crate) fn alloc_node(&mut self, value: T) -> usize {
        let index = if self.free_head == NIL {
            self.storage.grow()
        } else {
            self.pop_free()
        };

        self.len += 1;
        self.storage.data_mut(index).write(value);

        index
    }

    #[inline]
    pub(crate) fn free_node(&mut self, index: usize) -> T {
        let value = unsafe { self.storage.data_mut(index).assume_init_read() };

        self.len -= 1;
        self.push_free(index);

        if self.len == 0 {
            // 空表：端点归位（读 `head`/`tail` 的地方都能沿用旧形状）
            self.head = NIL;
            self.tail = NIL;
        }

        value
    }

    /// 摘掉一个 live 节点并回收它（两端都可能是链头 / 链尾，所以要判断）。
    #[inline]
    pub(crate) fn remove_node(&mut self, index: usize) -> T {
        let prev = self.storage.prev(index);
        let next = self.storage.next(index);

        if index == self.head {
            // 空表时会被 `free_node` 归位成 NIL
            self.head = next;
        } else {
            self.storage.set_next(prev, next);
        }

        if index == self.tail {
            self.tail = prev;
        } else {
            self.storage.set_prev(next, prev);
        }

        self.free_node(index)
    }

    /// 沿链向前走 `steps` 步。调用方保证 `steps < len`。
    #[inline]
    fn move_forward(&self, index: usize, steps: usize) -> usize {
        let mut current = index;

        for _ in 0..steps {
            current = self.storage.next(current);
        }

        current
    }

    /// 沿链向后走 `steps` 步。调用方保证 `steps < len`。
    #[inline]
    fn move_backward(&self, index: usize, steps: usize) -> usize {
        let mut current = index;

        for _ in 0..steps {
            current = self.storage.prev(current);
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

    // --------------------------------------------------------
    // 端操作
    // --------------------------------------------------------

    #[inline]
    fn push_front_index(&mut self, value: T) -> usize {
        let index = self.alloc_node(value);

        self.insert_front(index);

        index
    }

    #[inline]
    fn push_back_index(&mut self, value: T) -> usize {
        let index = self.alloc_node(value);

        self.insert_back(index);

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
        if self.len == 0 {
            return None;
        }

        let index = self.head;

        // 链头节点的 `prev` 是哑元，不用（也没有什么可）更新；摘空了会被归位
        self.head = self.storage.next(index);

        Some(self.free_node(index))
    }

    /// 对齐 `LinkedList::pop_back`。
    #[inline]
    pub fn pop_back(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        let index = self.tail;

        self.tail = self.storage.prev(index);

        Some(self.free_node(index))
    }

    // --------------------------------------------------------
    // 端点访问
    // --------------------------------------------------------

    /// 对齐 `LinkedList::front`。
    #[inline]
    pub fn front(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data(self.head).assume_init_ref() })
    }

    /// 对齐 `LinkedList::back`。
    #[inline]
    pub fn back(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data(self.tail).assume_init_ref() })
    }

    /// 对齐 `LinkedList::front_mut`。
    #[inline]
    pub fn front_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data_mut(self.head).assume_init_mut() })
    }

    /// 对齐 `LinkedList::back_mut`。
    #[inline]
    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
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
    ///
    /// 收尾用**固定次数**（进循环前把 `len` 记下来递减），不是"每轮判
    /// `len > 0` + `index == tail`"——后者每轮多一次比较，实测 1M 元素慢
    /// 3~13%（三个布局、三种删除比例里 8/9 更慢）。链两端的哑元不是 NIL，
    /// 所以遍历必须按个数走。
    #[inline]
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let mut index = self.head;
        // 按个数走：两端的哑元不是 NIL，链没有"天然终点"
        let mut remaining = self.len;

        while remaining > 0 {
            remaining -= 1;

            // 先取下一个：摘掉 `index` 会改写它自己的链接
            let next = self.storage.next(index);

            let keep = f(unsafe { self.storage.data_mut(index).assume_init_mut() });

            if !keep {
                let _ = self.remove_node(index);
            }

            index = next;
        }
    }

    /// 对齐 `LinkedList::append`：把 `other` 的**全部槽位**整块搬到末尾——搬槽位的
    /// 同时把下标批量加上偏移（索引只写一遍），两条链各接一次。
    /// 复杂度 O(other 的槽位总数)，与对方的活元素数无关。
    ///
    /// 语义：**对方的空闲槽也一起搬过来**；`other` 之后是"空表、容量留在它自己
    /// 那里、槽位已被搬空"。对方槽位数为 0 时直接返回。
    ///
    /// 想要"逐元素搬到末尾、优先复用自己已有的空闲槽"（按活元素计费），用
    /// [`append_elementwise`](List::append_elementwise)；两条路的取舍与实测见那里。
    pub fn append(&mut self, other: &mut Self) {
        let other_slots = other.storage.len();

        if other_slots == 0 {
            return;
        }

        let other_len = other.len;
        let self_free_empty = self.free_head == NIL;
        let base = self.storage.len();
        let other_head = other.head;
        let other_tail = other.tail;
        let other_free_head = other.free_head;
        let other_free_tail = other.free_tail;

        // 1) 搬槽位：数据整块搬，两条链的下标在搬的同时加好 base
        self.storage.append(&mut other.storage);

        // 2) live 链：把对方整条接在自己链尾后面
        if other_len > 0 {
            if self.len == 0 {
                self.head = other_head + base;
                self.tail = other_tail + base;
            } else {
                let tail = self.tail;

                self.storage.set_next(tail, other_head + base);
                self.storage.set_prev(other_head + base, tail);
                self.tail = other_tail + base;
            }
        }

        // 3) free 链：把自己链尾接上对方的链头（自己入口不变，保持 LIFO）
        if other_free_head != NIL {
            let other_free_head = other_free_head + base;

            if self_free_empty {
                self.free_head = other_free_head;
            } else {
                self.storage.set_next(self.free_tail, other_free_head);
            }

            self.free_tail = other_free_tail + base;
        }

        self.len += other_len;

        // 对方被搬空了：四个端点字段归位（不变量：字段是 NIL ⟺ 对应链为空）
        other.len = 0;
        other.head = NIL;
        other.tail = NIL;
        other.free_head = NIL;
        other.free_tail = NIL;
    }

    /// **本库扩展**：逐元素把 `other` 的所有元素搬到自己末尾（`other` 变空表）。
    ///
    /// 等价于 `while let Some(value) = other.pop_front() { self.push_back(value); }`：
    /// 优先填自己**已有的空闲槽**（已分配、已触碰，不产生新页），槽位不够才扩容。
    /// 代价按**活元素数**走，与对方的槽位数无关。
    ///
    /// 与 [`append`](List::append) 的取舍（1M 规模实测，本机当前状态）：
    ///
    /// | 形态 | 本方法 | `append` |
    /// |---|---|---|
    /// | 自己的空闲槽够装 + 对方密 | **~2.1 ms** | 4.4 ~ 4.9 ms（要扩容并搬旧数据） |
    /// | 自己的空闲槽够装 + 对方稀疏（1M 槽 / 100 活） | **~0.001 ms** | 3.4 ~ 4.4 ms（按槽位数搬） |
    /// | 自己装不下 + 对方密 | 4.7 ~ 4.9 ms（边塞边扩容） | **3.5 ms** |
    ///
    /// 也就是说：**自己有空闲槽、或对方稀疏时用它；否则用 `append`。**
    pub fn append_elementwise(&mut self, other: &mut Self) {
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

        self.remove_node(index)
    }

    /// 对齐 `LinkedList::clear`。保留已分配的容量与槽位。
    ///
    /// 单趟完成：逐个析构 live 元素，并把槽位就地挂回 free 链。循环条件用
    /// `len`（链尾的哑元不是 NIL，不能"走到 NIL 为止"）。
    ///
    /// 为什么不用"一直 `pop_front` 到空"：`pop_front` 每个元素都要走
    /// [`remove_node`](List::remove_node) / `free_node`——读 `prev`、修补邻居、
    /// 判断端点，还要 `assume_init_read()` **把 `T` 搬出槽位**再析构；`clear` 只读
    /// `next`、**就地** `assume_init_drop`、端点只归位一次。
    ///
    /// 实测（min/9 轮，`clear` 相对 pop 到空）：8 B 载荷 1.2×、64 B **2.9×**、
    /// 512 B **24.5×**——收益随 `T` 变大而增长（省掉的是每元素一次搬值）；带 Drop
    /// glue 时 drop 调用本身占大头，64 B 只剩 1.6×。
    #[inline]
    pub fn clear(&mut self) {
        while self.len > 0 {
            let index = self.head;

            // 先取下一个：`push_free` 会改写 `index` 自己的 `next`
            self.head = self.storage.next(index);

            unsafe { self.storage.data_mut(index).assume_init_drop() };

            self.len -= 1;
            self.push_free(index);
        }

        self.head = NIL;
        self.tail = NIL;
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
            free_tail: NIL,
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
