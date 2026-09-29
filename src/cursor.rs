use crate::list::List;

/// "逻辑位置未知"（由 [`Slot`](crate::Slot) 进入的游标）：`pos` 是走链走出来的，
/// 句柄里没有它。需要时**按需走一趟链**把它算出来（`O(len)`）。
pub(crate) const POS_UNKNOWN: usize = usize::MAX;

/// 从链头走到 `slot`，数出它的逻辑位置。只给"按槽位进入"的游标兜底用。
pub(crate) fn pos_of_slot<T, S: Storage<T>>(list: &List<T, S>, slot: usize) -> usize {
    let mut current = list.head;
    let mut pos = 0;

    while current != slot {
        current = list.storage.next(current);
        pos += 1;
        debug_assert!(pos < list.len, "槽位不在 live 链上");
    }

    pos
}
use crate::storage::{NIL, Storage};

/// 只读游标，对齐 `std::collections::linked_list::Cursor`。
///
/// 游标指向某个元素，或指向“幽灵”位置（`index()` / `current()` 返回
/// `None`）。空链表上的游标永远在幽灵位置。语义是环形的：从最后一个
/// 元素 `move_next()` 到幽灵，从幽灵 `move_next()` 到第一个元素。
pub struct Cursor<'a, T, S: Storage<T>> {
    pub(crate) list: &'a List<T, S>,
    /// 所指节点的下标；`NIL` 表示幽灵位置。
    pub(crate) slot: usize,
    /// 逻辑位置，与 std 一样缓存下来，使 `index()` 为 O(1)。
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> Cursor<'a, T, S> {
    pub(crate) fn at(list: &'a List<T, S>, slot: usize, pos: usize) -> Self {
        Self { list, slot, pos }
    }

    /// 对齐 `Cursor::index`。幽灵位置返回 `None`。
    ///
    /// 若这个游标是**按 [`Slot`](crate::Slot) 进入**的（`pos` 未知），这里会走一趟链
    /// 把它算出来：`O(len)`（正常按位置进入时是 `O(1)`）。
    pub fn index(&self) -> Option<usize> {
        if self.slot == NIL {
            None
        } else if self.pos == POS_UNKNOWN {
            Some(pos_of_slot(self.list, self.slot))
        } else {
            Some(self.pos)
        }
    }

    /// **本库扩展**：当前元素的稳定句柄（幽灵位置返回 `None`）。
    ///
    /// 与 [`index`](Self::index) 的区别：`index` 给**逻辑位置**（会随插删变），
    /// `slot` 给**物理槽位**（元素存活期内不变，可存进别的容器当 key）。
    pub fn slot(&self) -> Option<crate::Slot> {
        if self.slot == NIL {
            None
        } else {
            Some(crate::Slot(self.slot))
        }
    }

    /// 对齐 `Cursor::current`。幽灵位置返回 `None`。
    pub fn current(&self) -> Option<&'a T> {
        if self.slot == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data(self.slot).assume_init_ref() })
    }

    /// 对齐 `Cursor::peek_next`，不移动游标。
    pub fn peek_next(&self) -> Option<&'a T> {
        // 空表没有下一个；已经在链尾也没有（尾的 `next` 是哑元）
        if self.list.len == 0 || (self.slot != NIL && self.slot == self.list.tail) {
            return None;
        }

        let next = if self.slot == NIL {
            self.list.head
        } else {
            self.list.storage.next(self.slot)
        };

        Some(unsafe { self.list.storage.data(next).assume_init_ref() })
    }

    /// 对齐 `Cursor::peek_prev`，不移动游标。
    pub fn peek_prev(&self) -> Option<&'a T> {
        // 空表没有上一个；已经在链头也没有（链头的 `prev` 是哑元）
        if self.list.len == 0 || (self.slot != NIL && self.slot == self.list.head) {
            return None;
        }

        let prev = if self.slot == NIL {
            self.list.tail
        } else {
            self.list.storage.prev(self.slot)
        };

        Some(unsafe { self.list.storage.data(prev).assume_init_ref() })
    }

    /// 对齐 `Cursor::move_next`。
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // 空表上永远在幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.head;
            self.pos = 0;
        } else if self.slot == self.list.tail {
            // 越过链尾：进入幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.next(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        }
    }

    /// 对齐 `Cursor::move_prev`。
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.slot == self.list.head {
            // 越过链头：进入幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.prev(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos -= 1;
            }
        }
    }

    /// 对齐 `Cursor::front`。
    pub fn front(&self) -> Option<&'a T> {
        self.list.front()
    }

    /// 对齐 `Cursor::back`。
    pub fn back(&self) -> Option<&'a T> {
        self.list.back()
    }

    /// 对齐 `Cursor::as_list`。
    pub fn as_list(&self) -> &'a List<T, S> {
        self.list
    }
}

/// 可写游标，对齐 `std::collections::linked_list::CursorMut`；
/// 另有本库扩展 `seek` / `move_steps` / `is_head` / `is_tail`。
pub struct CursorMut<'a, T, S: Storage<T>> {
    pub(crate) list: &'a mut List<T, S>,
    pub(crate) slot: usize,
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> CursorMut<'a, T, S> {
    pub(crate) fn at(list: &'a mut List<T, S>, slot: usize, pos: usize) -> Self {
        Self { list, slot, pos }
    }

    /// 对齐 `CursorMut::index`。幽灵位置返回 `None`。
    ///
    /// 若游标是**按 [`Slot`](crate::Slot) 进入**的（`pos` 未知），会走一趟链把它算出来
    /// （`O(len)`）。
    pub fn index(&self) -> Option<usize> {
        if self.slot == NIL {
            None
        } else if self.pos == POS_UNKNOWN {
            Some(pos_of_slot(self.list, self.slot))
        } else {
            Some(self.pos)
        }
    }

    /// **本库扩展**：当前元素的稳定句柄（幽灵位置返回 `None`）。见 [`Cursor::slot`]。
    pub fn slot(&self) -> Option<crate::Slot> {
        if self.slot == NIL {
            None
        } else {
            Some(crate::Slot(self.slot))
        }
    }

    /// 对齐 `CursorMut::current`。幽灵位置返回 `None`。
    pub fn current(&mut self) -> Option<&mut T> {
        if self.slot == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data_mut(self.slot).assume_init_mut() })
    }

    /// 对齐 `CursorMut::peek_next`，不移动游标。
    pub fn peek_next(&mut self) -> Option<&mut T> {
        if self.list.len == 0 || (self.slot != NIL && self.slot == self.list.tail) {
            return None;
        }

        let next = if self.slot == NIL {
            self.list.head
        } else {
            self.list.storage.next(self.slot)
        };

        Some(unsafe { self.list.storage.data_mut(next).assume_init_mut() })
    }

    /// 对齐 `CursorMut::peek_prev`，不移动游标。
    pub fn peek_prev(&mut self) -> Option<&mut T> {
        if self.list.len == 0 || (self.slot != NIL && self.slot == self.list.head) {
            return None;
        }

        let prev = if self.slot == NIL {
            self.list.tail
        } else {
            self.list.storage.prev(self.slot)
        };

        Some(unsafe { self.list.storage.data_mut(prev).assume_init_mut() })
    }

    /// 对齐 `CursorMut::move_next`。
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // 空表上永远在幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.head;
            self.pos = 0;
        } else if self.slot == self.list.tail {
            // 越过链尾：进入幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.next(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        }
    }

    /// 对齐 `CursorMut::move_prev`。
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.slot == self.list.head {
            // 越过链头：进入幽灵位置
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.prev(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos -= 1;
            }
        }
    }

    /// 对齐 `CursorMut::insert_before`。幽灵位置插入到末尾。游标不动
    /// （仍指向原节点）。
    pub fn insert_before(&mut self, item: T) {
        let new = self.list.alloc_slot(item);

        if self.slot == NIL {
            // 幽灵位置：追加到末尾
            self.list.insert_back(new);
        } else if self.slot == self.list.head {
            // 插在链头之前：新节点成为链头（原链头的 `prev` 是哑元，不能用）
            self.list.insert_front(new);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        } else {
            let prev = self.list.storage.prev(self.slot);

            self.list.insert_between(new, prev, self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        }
    }

    /// 对齐 `CursorMut::insert_after`。幽灵位置插入到最前。游标不动。
    pub fn insert_after(&mut self, item: T) {
        let new = self.list.alloc_slot(item);

        if self.slot == NIL {
            // 幽灵位置：插到最前（空表时也是这一支）
            self.list.insert_front(new);
        } else if self.slot == self.list.tail {
            // 插在链尾之后：新节点成为链尾（原链尾的 `next` 是哑元）
            self.list.insert_back(new);
        } else {
            let next = self.list.storage.next(self.slot);

            self.list.insert_between(new, self.slot, next);
        }
    }

    /// 对齐 `CursorMut::remove_current_as_list`：把当前元素摘下来、当成一条**单元素链表**返回
    /// （游标语义与 [`remove_current`](Self::remove_current) 一致）。
    ///
    /// 与 std 的差别：std 把**节点本身**接过去（`O(1)`、句柄不变），这里是"摘下来再
    /// `push_back` 到新表" ⇒ 元素拿到**新槽位**，旧句柄作废。
    pub fn remove_current_as_list(&mut self) -> Option<List<T, S>>
    where
        S: Default,
    {
        let value = self.remove_current()?;

        let mut out = List::default();

        out.push_back(value);

        Some(out)
    }

    /// 对齐 `CursorMut::splice_before`：把 `list` 的元素**按原顺序**接到当前元素之前
    /// （幽灵位置 = 追加到末尾）。
    ///
    /// 与 std 的差别：std 是 `O(1)` 搬链（还要求两条链同分配器），这里是逐个
    /// `insert_before`，`O(list.len())`，且元素拿到**本表的新槽位**（旧句柄作废）。
    pub fn splice_before(&mut self, list: List<T, S>) {
        for value in list {
            self.insert_before(value);
        }
    }

    /// 对齐 `CursorMut::splice_after`：接到当前元素之后，**顺序保持**。
    ///
    /// （`insert_after` 每次都紧贴游标插，所以逆序喂进去才等于把 `list` 原样接在后面。）
    pub fn splice_after(&mut self, list: List<T, S>) {
        for value in list.into_iter().rev() {
            self.insert_after(value);
        }
    }

    /// 对齐 `CursorMut::split_before`：把当前元素**之前**的部分摘成一条新链表；游标留在
    /// 剩下的表头（原当前元素成为链头）。幽灵位置（尾部）⇒ 整条表都摘走。
    ///
    /// **复杂度不一样**：std 是 `O(1)`（切指针），我们是 `O(pos)` —— 槽位同处一块存储，
    /// 摘一半必须搬元素；**搬走的元素拿到新表的槽位，旧句柄作废**。
    pub fn split_before(&mut self) -> List<T, S>
    where
        S: Default,
    {
        let at = self.index().unwrap_or(self.list.len);

        let mut out = List::default();

        for _ in 0..at {
            // `at <= len` 由 `index()` 保证
            let value = self.list.pop_front().expect("at <= len");

            out.push_back(value);
        }

        if self.slot == NIL {
            self.pos = 0;
        } else if self.pos != POS_UNKNOWN {
            // 当前元素现在是链头
            self.pos = 0;
        }

        out
    }

    /// 对齐 `CursorMut::split_after`：把当前元素**之后**的部分摘成一条新链表
    /// （游标与剩余部分不动）。复杂度同样是 `O(len - pos)`，不是 std 的 `O(1)`。
    pub fn split_after(&mut self) -> List<T, S>
    where
        S: Default,
    {
        let at = self
            .index()
            .map_or(self.list.len, |pos| pos + 1)
            .min(self.list.len);

        self.list.split_off(at)
    }

    /// 对齐 `CursorMut::remove_current`：返回被删元素，游标移到下一个
    /// （删的是尾元素则移到幽灵位置）。幽灵位置返回 `None`。
    pub fn remove_current(&mut self) -> Option<T> {
        if self.slot == NIL {
            return None;
        }

        let slot = self.slot;
        let next = self.list.storage.next(slot);
        // 摘之前判断：摘掉链尾之后 `tail` 就变了
        let was_tail = slot == self.list.tail;

        let value = self.list.unlink_slot(slot);

        if was_tail {
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = next;
        }

        Some(value)
    }

    /// 对齐 `CursorMut::push_front`。游标指向的节点不变。
    pub fn push_front(&mut self, item: T) {
        self.list.push_front(item);

        if self.slot != NIL && self.pos != POS_UNKNOWN {
            self.pos += 1;
        }
    }

    /// 对齐 `CursorMut::push_back`。游标指向的节点不变。
    pub fn push_back(&mut self, item: T) {
        self.list.push_back(item);
    }

    /// 对齐 `CursorMut::pop_front`。若游标原本指向队首，则移到新队首。
    pub fn pop_front(&mut self) -> Option<T> {
        let front = self.list.head;
        let value = self.list.pop_front()?;

        if self.slot != NIL {
            if self.slot == front {
                self.slot = self.list.head;
                self.pos = 0;
            } else if self.pos != POS_UNKNOWN {
                self.pos -= 1;
            }
        }

        Some(value)
    }

    /// 对齐 `CursorMut::pop_back`。若游标原本指向队尾，则移到幽灵位置。
    pub fn pop_back(&mut self) -> Option<T> {
        let back = self.list.tail;
        let value = self.list.pop_back()?;

        if self.slot != NIL && self.slot == back {
            self.slot = NIL;
            self.pos = 0;
        }

        Some(value)
    }

    /// 对齐 `CursorMut::front`。
    pub fn front(&self) -> Option<&T> {
        self.list.front()
    }

    /// 对齐 `CursorMut::front_mut`。
    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.list.front_mut()
    }

    /// 对齐 `CursorMut::back`。
    pub fn back(&self) -> Option<&T> {
        self.list.back()
    }

    /// 对齐 `CursorMut::back_mut`。
    pub fn back_mut(&mut self) -> Option<&mut T> {
        self.list.back_mut()
    }

    /// 对齐 `CursorMut::as_cursor`。
    pub fn as_cursor(&self) -> Cursor<'_, T, S> {
        Cursor::at(self.list, self.slot, self.pos)
    }

    /// 对齐 `CursorMut::as_list`。
    pub fn as_list(&self) -> &List<T, S> {
        self.list
    }

    // --------------------------------------------------------
    // 本库扩展（std 没有）
    // --------------------------------------------------------

    /// **本库扩展**：移到第 `pos` 个元素，越界返回 `false` 且游标不动。
    /// O(min(pos, len - 1 - pos))。
    pub fn seek(&mut self, pos: usize) -> bool {
        if pos >= self.list.len {
            return false;
        }

        self.slot = self.list.slot_at(pos);
        self.pos = pos;

        true
    }

    /// **本库扩展**：相对移动 `offset` 步，越界返回 `false` 且游标不动。
    /// O(|offset|)；但若游标是**按 [`Slot`](crate::Slot) 进入**的（位置未知），
    /// 会先走一趟链算出当前位置：`O(len + |offset|)`。
    pub fn move_steps(&mut self, offset: isize) -> bool {
        if self.slot == NIL {
            return false;
        }

        if self.pos == POS_UNKNOWN {
            self.pos = pos_of_slot(self.list, self.slot);
        }

        let target = match self.pos.checked_add_signed(offset) {
            Some(target) if target < self.list.len => target,
            _ => return false,
        };

        self.slot = self.list.slot_at(target);
        self.pos = target;

        true
    }

    /// **本库扩展**：是否指向队首元素。O(1)。
    pub fn is_head(&self) -> bool {
        self.slot != NIL && self.slot == self.list.head
    }

    /// **本库扩展**：是否指向队尾元素。O(1)。
    pub fn is_tail(&self) -> bool {
        self.slot != NIL && self.slot == self.list.tail
    }
}
