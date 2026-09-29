use crate::list::List;
use crate::storage::{NIL, Storage};

/// 只读游标，对齐 `std::collections::linked_list::Cursor`。
///
/// 游标指向某个元素，或指向“幽灵”位置（`index()` / `current()` 返回
/// `None`）。空链表上的游标永远在幽灵位置。语义是环形的：从最后一个
/// 元素 `move_next()` 到幽灵，从幽灵 `move_next()` 到第一个元素。
pub struct Cursor<'a, T, S: Storage<T>> {
    pub(crate) list: &'a List<T, S>,
    /// 所指节点的下标；`NIL` 表示幽灵位置。
    pub(crate) node: usize,
    /// 逻辑位置，与 std 一样缓存下来，使 `index()` 为 O(1)。
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> Cursor<'a, T, S> {
    pub(crate) fn at(list: &'a List<T, S>, node: usize, pos: usize) -> Self {
        Self { list, node, pos }
    }

    /// 对齐 `Cursor::index`。幽灵位置返回 `None`。
    pub fn index(&self) -> Option<usize> {
        if self.node == NIL {
            None
        } else {
            Some(self.pos)
        }
    }

    /// 对齐 `Cursor::current`。幽灵位置返回 `None`。
    pub fn current(&self) -> Option<&'a T> {
        if self.node == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data(self.node).assume_init_ref() })
    }

    /// 对齐 `Cursor::peek_next`，不移动游标。
    pub fn peek_next(&self) -> Option<&'a T> {
        // 空表没有下一个；已经在链尾也没有（尾的 `next` 是哑元）
        if self.list.len == 0 || (self.node != NIL && self.node == self.list.tail) {
            return None;
        }

        let next = if self.node == NIL {
            self.list.head
        } else {
            self.list.storage.next(self.node)
        };

        Some(unsafe { self.list.storage.data(next).assume_init_ref() })
    }

    /// 对齐 `Cursor::peek_prev`，不移动游标。
    pub fn peek_prev(&self) -> Option<&'a T> {
        // 空表没有上一个；已经在链头也没有（链头的 `prev` 是哑元）
        if self.list.len == 0 || (self.node != NIL && self.node == self.list.head) {
            return None;
        }

        let prev = if self.node == NIL {
            self.list.tail
        } else {
            self.list.storage.prev(self.node)
        };

        Some(unsafe { self.list.storage.data(prev).assume_init_ref() })
    }

    /// 对齐 `Cursor::move_next`。
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // 空表上永远在幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else if self.node == NIL {
            self.node = self.list.head;
            self.pos = 0;
        } else if self.node == self.list.tail {
            // 越过链尾：进入幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else {
            self.node = self.list.storage.next(self.node);
            self.pos += 1;
        }
    }

    /// 对齐 `Cursor::move_prev`。
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.node = NIL;
            self.pos = 0;
        } else if self.node == NIL {
            self.node = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.node == self.list.head {
            // 越过链头：进入幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else {
            self.node = self.list.storage.prev(self.node);
            self.pos -= 1;
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
    pub(crate) node: usize,
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> CursorMut<'a, T, S> {
    pub(crate) fn at(list: &'a mut List<T, S>, node: usize, pos: usize) -> Self {
        Self { list, node, pos }
    }

    /// 对齐 `CursorMut::index`。幽灵位置返回 `None`。
    pub fn index(&self) -> Option<usize> {
        if self.node == NIL {
            None
        } else {
            Some(self.pos)
        }
    }

    /// 对齐 `CursorMut::current`。幽灵位置返回 `None`。
    pub fn current(&mut self) -> Option<&mut T> {
        if self.node == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data_mut(self.node).assume_init_mut() })
    }

    /// 对齐 `CursorMut::peek_next`，不移动游标。
    pub fn peek_next(&mut self) -> Option<&mut T> {
        if self.list.len == 0 || (self.node != NIL && self.node == self.list.tail) {
            return None;
        }

        let next = if self.node == NIL {
            self.list.head
        } else {
            self.list.storage.next(self.node)
        };

        Some(unsafe { self.list.storage.data_mut(next).assume_init_mut() })
    }

    /// 对齐 `CursorMut::peek_prev`，不移动游标。
    pub fn peek_prev(&mut self) -> Option<&mut T> {
        if self.list.len == 0 || (self.node != NIL && self.node == self.list.head) {
            return None;
        }

        let prev = if self.node == NIL {
            self.list.tail
        } else {
            self.list.storage.prev(self.node)
        };

        Some(unsafe { self.list.storage.data_mut(prev).assume_init_mut() })
    }

    /// 对齐 `CursorMut::move_next`。
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // 空表上永远在幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else if self.node == NIL {
            self.node = self.list.head;
            self.pos = 0;
        } else if self.node == self.list.tail {
            // 越过链尾：进入幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else {
            self.node = self.list.storage.next(self.node);
            self.pos += 1;
        }
    }

    /// 对齐 `CursorMut::move_prev`。
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.node = NIL;
            self.pos = 0;
        } else if self.node == NIL {
            self.node = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.node == self.list.head {
            // 越过链头：进入幽灵位置
            self.node = NIL;
            self.pos = 0;
        } else {
            self.node = self.list.storage.prev(self.node);
            self.pos -= 1;
        }
    }

    /// 对齐 `CursorMut::insert_before`。幽灵位置插入到末尾。游标不动
    /// （仍指向原节点）。
    pub fn insert_before(&mut self, item: T) {
        let new = self.list.alloc_node(item);

        if self.node == NIL {
            // 幽灵位置：追加到末尾
            self.list.insert_back(new);
        } else if self.node == self.list.head {
            // 插在链头之前：新节点成为链头（原链头的 `prev` 是哑元，不能用）
            self.list.insert_front(new);

            self.pos += 1;
        } else {
            let prev = self.list.storage.prev(self.node);

            self.list.insert_between(new, prev, self.node);

            self.pos += 1;
        }
    }

    /// 对齐 `CursorMut::insert_after`。幽灵位置插入到最前。游标不动。
    pub fn insert_after(&mut self, item: T) {
        let new = self.list.alloc_node(item);

        if self.node == NIL {
            // 幽灵位置：插到最前（空表时也是这一支）
            self.list.insert_front(new);
        } else if self.node == self.list.tail {
            // 插在链尾之后：新节点成为链尾（原链尾的 `next` 是哑元）
            self.list.insert_back(new);
        } else {
            let next = self.list.storage.next(self.node);

            self.list.insert_between(new, self.node, next);
        }
    }

    /// 对齐 `CursorMut::remove_current`：返回被删元素，游标移到下一个
    /// （删的是尾元素则移到幽灵位置）。幽灵位置返回 `None`。
    pub fn remove_current(&mut self) -> Option<T> {
        if self.node == NIL {
            return None;
        }

        let node = self.node;
        let next = self.list.storage.next(node);
        // 摘之前判断：摘掉链尾之后 `tail` 就变了
        let was_tail = node == self.list.tail;

        let value = self.list.remove_node(node);

        if was_tail {
            self.node = NIL;
            self.pos = 0;
        } else {
            self.node = next;
        }

        Some(value)
    }

    /// 对齐 `CursorMut::push_front`。游标指向的节点不变。
    pub fn push_front(&mut self, item: T) {
        self.list.push_front(item);

        if self.node != NIL {
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

        if self.node != NIL {
            if self.node == front {
                self.node = self.list.head;
                self.pos = 0;
            } else {
                self.pos -= 1;
            }
        }

        Some(value)
    }

    /// 对齐 `CursorMut::pop_back`。若游标原本指向队尾，则移到幽灵位置。
    pub fn pop_back(&mut self) -> Option<T> {
        let back = self.list.tail;
        let value = self.list.pop_back()?;

        if self.node != NIL && self.node == back {
            self.node = NIL;
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
        Cursor::at(self.list, self.node, self.pos)
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

        self.node = self.list.index_at(pos);
        self.pos = pos;

        true
    }

    /// **本库扩展**：相对移动 `offset` 步，越界返回 `false` 且游标不动。
    /// O(|offset|)。
    pub fn move_steps(&mut self, offset: isize) -> bool {
        if self.node == NIL {
            return false;
        }

        let target = match self.pos.checked_add_signed(offset) {
            Some(target) if target < self.list.len => target,
            _ => return false,
        };

        self.node = self.list.index_at(target);
        self.pos = target;

        true
    }

    /// **本库扩展**：是否指向队首元素。O(1)。
    pub fn is_head(&self) -> bool {
        self.node != NIL && self.node == self.list.head
    }

    /// **本库扩展**：是否指向队尾元素。O(1)。
    pub fn is_tail(&self) -> bool {
        self.node != NIL && self.node == self.list.tail
    }
}
