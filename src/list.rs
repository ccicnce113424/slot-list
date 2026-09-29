use std::fmt;
use std::marker::PhantomData;

use crate::cursor::{Cursor, CursorMut};
use crate::iter::{IntoIter, Iter, IterMut};
use crate::slot::Slot;
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
/// 一个字段（1 次 load + 比较），而不是算 `storage.slots() - len`（2 次 load +
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
    /// 只为"用上 `T`"而存在：`List` 是对**任意** `S: Storage<T>` 定义的，而 `T` 只出现在
    /// 这条 bound 里——Rust **不把 where 子句里的出现算作"被使用"**，删掉这个字段就是
    /// `error[E0392]: type parameter `T` is never used`（实测；注意 `SoaList<T> = List<T, Soa<T>>`
    /// 这种**具体别名**里 `T` 会经由 `Soa<T>` 进到字段类型，所以别名不受影响，受影响的是
    /// 泛型定义本身）。
    ///
    /// 选 `PhantomData<T>` 而不是别的 marker，是因为它的含义恰好与真实情况一致：
    ///
    /// - **拥有 `T`**：`List<T, S>` 析构时确实会析构 `T`（[`Drop`](List#impl-Drop) 走 live 链
    ///   逐个 `assume_init_drop`），dropck 需要知道这一点；
    /// - **对 `T` 协变**：与三种 `Storage`（`Vec<MaybeUninit<T>>` / `Link<T>` / `Node<T>`）一致
    ///   ⇒ `SoaList<&'static str>` 能当 `SoaList<&'a str>` 用；
    /// - **auto traits 跟着 `T`**：`List<T, S>: Send` 当且仅当 `T: Send`（`Sync` 同理）。
    ///
    /// 后两条有编译期测试（`tests::auto_traits_and_variance`）。
    marker: PhantomData<T>,
}

impl<T, I: crate::Ix> List<T, Soa<T, I>> {
    /// 空链表（SoA 布局；索引宽度由 `Soa` 的第二个参数决定）。
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

impl<T, I: crate::Ix> List<T, Packed<T, I>> {
    /// 空链表（Packed 布局；索引宽度由 `Packed` 的第二个参数决定）。
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

impl<T, I: crate::Ix> List<T, Aos<T, I>> {
    /// 空链表（AoS 布局；索引宽度由 `Aos` 的第二个参数决定）。
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

    /// 把 `pos` 插成链头。调用前 `pos` 已经算进 `len`（[`alloc_slot`](Self::alloc_slot) 干过）。
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
            // 链头的 `prev` 是哑元（没人读它指向谁），但**必须写**：它同时是
            // "槽位是 live 的"标记位所在位置，留着 free 链上的旧值会让这个
            // 新链头看起来还是空闲的。
            self.storage.set_prev(pos, pos);
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
        let slot = self.free_head;
        let next = self.storage.next(slot);

        if next == slot {
            // 只剩它一个（自环终止）⇒ 链空了：归位
            self.free_head = NIL;
            self.free_tail = NIL;
        } else {
            self.free_head = next;
        }

        slot
    }

    /// 把槽位挂回 free 链头。
    #[inline]
    fn push_free(&mut self, slot: usize) {
        // 唯一需要显式改标记位的地方：从这里起它不再是 live 槽位
        self.storage.mark_free(slot);

        if self.free_head == NIL {
            // 原本是空链：它同时也是链尾，`next` 写自环（NIL 决不能进数组）
            self.free_tail = slot;
            self.storage.set_next(slot, slot);
        } else {
            self.storage.set_next(slot, self.free_head);
        }

        self.free_head = slot;
    }

    #[inline]
    pub(crate) fn alloc_slot(&mut self, value: T) -> usize {
        let slot = if self.free_head == NIL {
            self.storage.grow()
        } else {
            self.pop_free()
        };

        self.len += 1;
        self.storage.data_mut(slot).write(value);

        slot
    }

    #[inline]
    pub(crate) fn free_slot(&mut self, slot: usize) -> T {
        let value = unsafe { self.storage.data_mut(slot).assume_init_read() };

        self.len -= 1;
        self.push_free(slot);

        if self.len == 0 {
            // 空表：端点归位（读 `head`/`tail` 的地方都能沿用旧形状）
            self.head = NIL;
            self.tail = NIL;
        }

        value
    }

    /// 把 live 槽位从链上摘下来（**不改 `len`、不回收**）。两端都可能是链头 / 链尾，
    /// 所以要判断。给"移动元素"（[`move_to_front`](Self::move_to_front)）与
    /// `unlink_slot` 共用。
    #[inline]
    fn detach(&mut self, slot: usize) {
        let prev = self.storage.prev(slot);
        let next = self.storage.next(slot);

        if slot == self.head {
            // 空表时会被 `free_slot` 归位成 NIL
            self.head = next;
        } else {
            self.storage.set_next(prev, next);
        }

        if slot == self.tail {
            self.tail = prev;
        } else {
            self.storage.set_prev(next, prev);
        }
    }

    /// 摘掉一个 live 节点并回收它。
    #[inline]
    pub(crate) fn unlink_slot(&mut self, slot: usize) -> T {
        self.detach(slot);
        self.free_slot(slot)
    }

    /// 句柄校验：在范围内**且**不是空闲槽。**不碰 `data`**（空闲槽的 `data` 未初始化）。
    #[inline]
    fn live_slot(&self, slot: Slot) -> Option<usize> {
        let slot = slot.0;

        (slot < self.storage.slots() && !self.storage.is_free(slot)).then_some(slot)
    }

    /// 沿链向前走 `steps` 步。调用方保证 `steps < len`。
    #[inline]
    fn move_forward(&self, slot: usize, steps: usize) -> usize {
        let mut current = slot;

        for _ in 0..steps {
            current = self.storage.next(current);
        }

        current
    }

    /// 沿链向后走 `steps` 步。调用方保证 `steps < len`。
    #[inline]
    fn move_backward(&self, slot: usize, steps: usize) -> usize {
        let mut current = slot;

        for _ in 0..steps {
            current = self.storage.prev(current);
        }

        current
    }

    /// 第 `pos` 个 live 节点的下标；`pos` 必须 `< len`。
    /// 从**步数更少**的一端出发：forward 走 `pos` 步，backward 走 `len-1-pos` 步。
    #[inline]
    pub(crate) fn slot_at(&self, pos: usize) -> usize {
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
    fn push_front_slot(&mut self, value: T) -> usize {
        let slot = self.alloc_slot(value);

        self.insert_front(slot);

        slot
    }

    #[inline]
    fn push_back_slot(&mut self, value: T) -> usize {
        let slot = self.alloc_slot(value);

        self.insert_back(slot);

        slot
    }

    /// 对齐 `LinkedList::push_front`。
    #[inline]
    pub fn push_front(&mut self, value: T) {
        let _ = self.push_front_slot(value);
    }

    /// 对齐 `LinkedList::push_back`。
    #[inline]
    pub fn push_back(&mut self, value: T) {
        let _ = self.push_back_slot(value);
    }

    /// 对齐 `LinkedList::push_front_mut`：插入并返回新元素的引用。
    #[inline]
    pub fn push_front_mut(&mut self, value: T) -> &mut T {
        let slot = self.push_front_slot(value);

        unsafe { self.storage.data_mut(slot).assume_init_mut() }
    }

    /// 对齐 `LinkedList::push_back_mut`：插入并返回新元素的引用。
    #[inline]
    pub fn push_back_mut(&mut self, value: T) -> &mut T {
        let slot = self.push_back_slot(value);

        unsafe { self.storage.data_mut(slot).assume_init_mut() }
    }

    /// 对齐 `LinkedList::pop_front`。
    #[inline]
    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        let slot = self.head;

        // 链头节点的 `prev` 是哑元，不用（也没有什么可）更新；摘空了会被归位
        self.head = self.storage.next(slot);

        Some(self.free_slot(slot))
    }

    /// 对齐 `LinkedList::pop_back`。
    #[inline]
    pub fn pop_back(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        let slot = self.tail;

        self.tail = self.storage.prev(slot);

        Some(self.free_slot(slot))
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

    /// **本库扩展**：按稳定句柄取游标，**O(1)**。句柄失效（槽位已空闲 / 越界）返回 `None`。
    ///
    /// 用它进入的游标**不知道自己的逻辑位置**：`index()` / `move_steps` 需要时才会走
    /// 一趟链算出来（`O(len)`），而按句柄的增删/搬移都是 `O(1)`。
    ///
    /// 不做世代校验：槽位被复用后，旧句柄会指向**新元素**（见 [`Slot`] 的文档）。
    pub fn cursor_at(&self, slot: Slot) -> Option<Cursor<'_, T, S>> {
        self.live_slot(slot)
            .map(|slot| Cursor::at(self, slot, crate::cursor::POS_UNKNOWN))
    }

    /// **本库扩展**：按稳定句柄取**可变**游标，**O(1)**。语义同 [`cursor_at`](Self::cursor_at)。
    pub fn cursor_at_mut(&mut self, slot: Slot) -> Option<CursorMut<'_, T, S>> {
        self.live_slot(slot)
            .map(|slot| CursorMut::at(self, slot, crate::cursor::POS_UNKNOWN))
    }

    /// **本库扩展**：按句柄删除并返回元素，**O(1)**（不需要逻辑位置）。句柄失效返回 `None`。
    pub fn remove_slot(&mut self, slot: Slot) -> Option<T> {
        self.live_slot(slot).map(|slot| self.unlink_slot(slot))
    }

    /// **本库扩展**：链头元素的句柄，`O(1)`。
    pub fn front_slot(&self) -> Option<Slot> {
        (self.head != NIL).then_some(Slot(self.head))
    }

    /// **本库扩展**：链尾元素的句柄，`O(1)`。
    pub fn back_slot(&self) -> Option<Slot> {
        (self.tail != NIL).then_some(Slot(self.tail))
    }

    /// **本库扩展**：把句柄指的元素搬到链头，**O(1)**（LRU 的原语之一）。
    /// 句柄失效返回 `None`；已经在链头则是空操作。
    pub fn move_to_front(&mut self, slot: Slot) -> Option<()> {
        let slot = self.live_slot(slot)?;

        if slot != self.head {
            self.detach(slot);
            self.insert_front(slot);
        }

        Some(())
    }

    /// **本库扩展**：把句柄指的元素搬到链尾，**O(1)**。语义同 [`move_to_front`](Self::move_to_front)。
    pub fn move_to_back(&mut self, slot: Slot) -> Option<()> {
        let slot = self.live_slot(slot)?;

        if slot != self.tail {
            self.detach(slot);
            self.insert_back(slot);
        }

        Some(())
    }

    /// **本库扩展**：句柄的逻辑位置，**O(len)**（要数一遍）。句柄失效返回 `None`。
    /// 只是偶尔想知道"第几个"时够用；常问就用 [`cursor_at`](Self::cursor_at) 拿游标。
    pub fn pos_of(&self, slot: Slot) -> Option<usize> {
        self.live_slot(slot)
            .map(|slot| crate::cursor::pos_of_slot(self, slot))
    }

    /// **本库扩展**：边迭代边给出每个元素的稳定句柄 `(Slot, &T)`。
    ///
    /// 想把"链表顺序"和"自己的哈希表"接起来时用它：句柄可以当 key 存起来，
    /// 之后用 [`cursor_at`](Self::cursor_at) / [`remove_slot`](Self::remove_slot) 回到
    /// 那个元素（`O(1)`）。
    pub fn iter_slots(&self) -> impl Iterator<Item = (Slot, &T)> + '_ {
        let mut slot = self.head;

        (0..self.len).map(move |_| {
            let current = slot;
            slot = self.storage.next(current);

            (Slot(current), unsafe {
                self.storage.data(current).assume_init_ref()
            })
        })
    }

    /// **本库扩展**（std 没有随机访问）：定位到第 `pos` 个元素。
    /// 从较近的一端出发，O(min(pos, len - 1 - pos))。
    #[inline]
    pub fn at(&mut self, pos: usize) -> Option<CursorMut<'_, T, S>> {
        if pos >= self.len {
            return None;
        }

        let slot = self.slot_at(pos);

        Some(CursorMut::at(self, slot, pos))
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
        let mut slot = self.head;
        // 按个数走：两端的哑元不是 NIL，链没有"天然终点"
        let mut remaining = self.len;

        while remaining > 0 {
            remaining -= 1;

            // 先取下一个：摘掉 `index` 会改写它自己的链接
            let next = self.storage.next(slot);

            let keep = f(unsafe { self.storage.data_mut(slot).assume_init_mut() });

            if !keep {
                let _ = self.unlink_slot(slot);
            }

            slot = next;
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
        let other_slots = other.storage.slots();

        if other_slots == 0 {
            return;
        }

        let other_len = other.len;
        let self_free_empty = self.free_head == NIL;
        let base = self.storage.slots();
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
        assert!(at <= self.len, "split_off slot out of bounds");

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
        assert!(at < self.len, "remove slot out of bounds");

        let slot = self.slot_at(at);

        self.unlink_slot(slot)
    }

    /// 对齐 `LinkedList::clear`。保留已分配的容量与槽位。
    ///
    /// 单趟完成：逐个析构 live 元素，并把槽位就地挂回 free 链。循环条件用
    /// `len`（链尾的哑元不是 NIL，不能"走到 NIL 为止"）。
    ///
    /// 为什么不用"一直 `pop_front` 到空"：`pop_front` 每个元素都要走
    /// [`unlink_slot`](List::unlink_slot) / `free_slot`——读 `prev`、修补邻居、
    /// 判断端点，还要 `assume_init_read()` **把 `T` 搬出槽位**再析构；`clear` 只读
    /// `next`、**就地** `assume_init_drop`、端点只归位一次。
    ///
    /// 实测（min/9 轮，`clear` 相对 pop 到空）：8 B 载荷 1.2×、64 B **2.9×**、
    /// 512 B **24.5×**——收益随 `T` 变大而增长（省掉的是每元素一次搬值）；带 Drop
    /// glue 时 drop 调用本身占大头，64 B 只剩 1.6×。
    ///
    /// # 为什么不做成"按内存顺序扫 `data`、用 `is_free` 判断"
    ///
    /// 试过（`benches/list.rs` 的 `clear_drop` 组，1M 槽位）：
    ///
    /// | 形状 | 追链表 | 扫描 | |
    /// |---|---|---|---|
    /// | 密集 `slots == len == 1M` | 1.150 ms | **500 µs** | 扫描快 2.3× |
    /// | 稀疏 `slots = 1M, len = 1000` | **1.42 µs** | 464 µs | 扫描慢 **327×** |
    /// | 密集 + Drop glue（64 B） | **3.215 ms** | 3.597 ms | 扫描慢 12% |
    /// | `clear` 后重填（复用局部性） | 2.343 ms | 2.346 ms | 无差别 |
    ///
    /// 扫描只有在"没有 Drop glue **且** 几乎满"时才赢，却把复杂度从 O(live)
    /// 换成 O(slots)：`clear` 一个刚排空的 deque 会从微秒级掉到几百微秒。带
    /// Drop glue 时（真实载荷）连密集形状都输——析构本身把带宽吃完，扫描多读的
    /// 那遍 `prev`（`is_free`）就是纯亏。free 链顺序换成升序也没换来复用收益。
    #[inline]
    pub fn clear(&mut self) {
        while self.len > 0 {
            let slot = self.head;

            // 先取下一个：`push_free` 会改写 `index` 自己的 `next`
            self.head = self.storage.next(slot);

            unsafe { self.storage.data_mut(slot).assume_init_drop() };

            self.len -= 1;
            self.push_free(slot);
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

/// 析构：只走 live 链、**就地**析构，不碰 free 链。
///
/// 不走 [`clear`](List::clear) 的原因：`clear` 还要把每个槽位挂回 free 链
/// （`mark_free` + `set_next` + 端点归位），而这里整个存储马上要跟着释放，
/// 那份维护是纯亏。实测（1M 元素、`clear_drop` 组）：
///
/// | 载荷 | 经 `clear` | 只析构 | |
/// |---|---|---|---|
/// | `usize`（无 glue，纯记账） | 1.172 ms | **1.86 µs** | 消掉白做 |
/// | `Drop64`（有 glue，Soa） | 3.134 ms | **2.117 ms** | 快 1.48× |
/// | `Drop64`（有 glue，Aos） | 2.835 ms | 2.509 ms | 快 1.13× |
///
/// 这一条对 `into_iter()` 半途丢弃同样有效（[`IntoIter`] 是 `pop_front` 的
/// 薄包装，剩余元素最终由这里收尾）。
///
/// 按 `len` 计数而不是"走到 NIL"：链尾的 `next` 是哑元（自环，或空闲期的残留）。
impl<T, S: Storage<T>> Drop for List<T, S> {
    fn drop(&mut self) {
        let mut slot = self.head;

        for _ in 0..self.len {
            let next = self.storage.next(slot);

            unsafe { self.storage.data_mut(slot).assume_init_drop() };

            slot = next;
        }
    }
}
