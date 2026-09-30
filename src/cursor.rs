use crate::list::List;

/// "Logical position unknown" (a cursor entered via [`Slot`](crate::Slot)): `pos`
/// comes from walking the chain; the handle does not carry it, so it is computed on demand (`O(len)`).
pub(crate) const POS_UNKNOWN: usize = usize::MAX;

/// Walk from the chain head to `slot` and count out its logical position. Only a fallback for cursors entered "by slot".
pub(crate) fn pos_of_slot<T, S: Storage<T>>(list: &List<T, S>, slot: usize) -> usize {
    let mut current = list.head;
    let mut pos = 0;

    while current != slot {
        current = list.storage.next(current);
        pos += 1;
        debug_assert!(pos < list.len, "slot is not on the live chain");
    }

    pos
}
use crate::storage::{NIL, Storage};

/// Read-only cursor, mirroring `std::collections::linked_list::Cursor`.
///
/// A cursor points at an element or at the "ghost" position (`index()` /
/// `current()` return `None`); a cursor on an empty list is always at the ghost.
/// Semantics are circular: `move_next()` from the last element goes to the ghost, and from the ghost to the first element.
pub struct Cursor<'a, T, S: Storage<T>> {
    pub(crate) list: &'a List<T, S>,
    /// Index of the pointed-at node; `NIL` means the ghost position.
    pub(crate) slot: usize,
    /// Logical position, cached like std so that `index()` is O(1).
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> Cursor<'a, T, S> {
    pub(crate) fn at(list: &'a List<T, S>, slot: usize, pos: usize) -> Self {
        Self { list, slot, pos }
    }

    /// Mirrors `Cursor::index`. Returns `None` at the ghost position.
    ///
    /// If this cursor was **entered via [`Slot`](crate::Slot)** (`pos` unknown),
    /// this walks the chain to compute it: `O(len)` (`O(1)` when entered normally by position).
    pub fn index(&self) -> Option<usize> {
        if self.slot == NIL {
            None
        } else if self.pos == POS_UNKNOWN {
            Some(pos_of_slot(self.list, self.slot))
        } else {
            Some(self.pos)
        }
    }

    /// **Library extension**: stable handle of the current element (`None` at the ghost position).
    ///
    /// Differs from [`index`](Self::index): `index` gives the **logical position**
    /// (changes with insertions/removals); `slot` gives the **physical slot** (constant while the element is alive, usable as a key elsewhere).
    pub fn slot(&self) -> Option<crate::Slot> {
        if self.slot == NIL {
            None
        } else {
            Some(crate::Slot(self.slot))
        }
    }

    /// Mirrors `Cursor::current`. Returns `None` at the ghost position.
    pub fn current(&self) -> Option<&'a T> {
        if self.slot == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data(self.slot).assume_init_ref() })
    }

    /// Mirrors `Cursor::peek_next`; does not move the cursor.
    pub fn peek_next(&self) -> Option<&'a T> {
        // No next on an empty list; none at the chain tail either (the tail's `next` is a dummy link)
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

    /// Mirrors `Cursor::peek_prev`; does not move the cursor.
    pub fn peek_prev(&self) -> Option<&'a T> {
        // No prev on an empty list; none at the chain head either (the head's `prev` is a dummy link)
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

    /// Mirrors `Cursor::move_next`.
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // Always at the ghost position on an empty list
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.head;
            self.pos = 0;
        } else if self.slot == self.list.tail {
            // Past the chain tail: enter the ghost position
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.next(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        }
    }

    /// Mirrors `Cursor::move_prev`.
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.slot == self.list.head {
            // Past the chain head: enter the ghost position
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.prev(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos -= 1;
            }
        }
    }

    /// Mirrors `Cursor::front`.
    pub fn front(&self) -> Option<&'a T> {
        self.list.front()
    }

    /// Mirrors `Cursor::back`.
    pub fn back(&self) -> Option<&'a T> {
        self.list.back()
    }

    /// Mirrors `Cursor::as_list`.
    pub fn as_list(&self) -> &'a List<T, S> {
        self.list
    }
}

/// Mutable cursor, mirroring `std::collections::linked_list::CursorMut`;
/// additionally provides the library extensions `seek` / `move_steps` / `is_head` / `is_tail`.
pub struct CursorMut<'a, T, S: Storage<T>> {
    pub(crate) list: &'a mut List<T, S>,
    pub(crate) slot: usize,
    pub(crate) pos: usize,
}

impl<'a, T, S: Storage<T>> CursorMut<'a, T, S> {
    pub(crate) fn at(list: &'a mut List<T, S>, slot: usize, pos: usize) -> Self {
        Self { list, slot, pos }
    }

    /// Mirrors `CursorMut::index`. Returns `None` at the ghost position.
    ///
    /// If the cursor was **entered via [`Slot`](crate::Slot)** (`pos` unknown), this
    /// walks the chain to compute it (`O(len)`).
    pub fn index(&self) -> Option<usize> {
        if self.slot == NIL {
            None
        } else if self.pos == POS_UNKNOWN {
            Some(pos_of_slot(self.list, self.slot))
        } else {
            Some(self.pos)
        }
    }

    /// **Library extension**: stable handle of the current element (`None` at the ghost position). See [`Cursor::slot`].
    pub fn slot(&self) -> Option<crate::Slot> {
        if self.slot == NIL {
            None
        } else {
            Some(crate::Slot(self.slot))
        }
    }

    /// Mirrors `CursorMut::current`. Returns `None` at the ghost position.
    pub fn current(&mut self) -> Option<&mut T> {
        if self.slot == NIL {
            return None;
        }

        Some(unsafe { self.list.storage.data_mut(self.slot).assume_init_mut() })
    }

    /// Mirrors `CursorMut::peek_next`; does not move the cursor.
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

    /// Mirrors `CursorMut::peek_prev`; does not move the cursor.
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

    /// Mirrors `CursorMut::move_next`.
    pub fn move_next(&mut self) {
        if self.list.len == 0 {
            // Always at the ghost position on an empty list
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.head;
            self.pos = 0;
        } else if self.slot == self.list.tail {
            // Past the chain tail: enter the ghost position
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.next(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos += 1;
            }
        }
    }

    /// Mirrors `CursorMut::move_prev`.
    pub fn move_prev(&mut self) {
        if self.list.len == 0 {
            self.slot = NIL;
            self.pos = 0;
        } else if self.slot == NIL {
            self.slot = self.list.tail;
            self.pos = self.list.len - 1;
        } else if self.slot == self.list.head {
            // Past the chain head: enter the ghost position
            self.slot = NIL;
            self.pos = 0;
        } else {
            self.slot = self.list.storage.prev(self.slot);

            if self.pos != POS_UNKNOWN {
                self.pos -= 1;
            }
        }
    }

    /// Mirrors `CursorMut::insert_before`. At the ghost position, inserts at the
    /// back. The cursor does not move (it still points at the original node).
    pub fn insert_before(&mut self, item: T) {
        let new = self.list.alloc_slot(item);

        if self.slot == NIL {
            // Ghost position: append at the back
            self.list.insert_back(new);
        } else if self.slot == self.list.head {
            // Insert before the chain head: the new node becomes the head (the old head's `prev` is a dummy link and cannot be used)
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

    /// Mirrors `CursorMut::insert_after`. At the ghost position, inserts at the front. The cursor does not move.
    pub fn insert_after(&mut self, item: T) {
        let new = self.list.alloc_slot(item);

        if self.slot == NIL {
            // Ghost position: insert at the front (this branch also covers an empty list)
            self.list.insert_front(new);
        } else if self.slot == self.list.tail {
            // Insert after the chain tail: the new node becomes the tail (the old tail's `next` is a dummy link)
            self.list.insert_back(new);
        } else {
            let next = self.list.storage.next(self.slot);

            self.list.insert_between(new, self.slot, next);
        }
    }

    /// Mirrors `CursorMut::remove_current_as_list`: detaches the current element and
    /// returns it as a **single-element list** (cursor semantics match [`remove_current`](Self::remove_current)).
    ///
    /// Differs from std: std splices the **node itself** over (`O(1)`, handle unchanged),
    /// whereas here the element is detached and `push_back`ed into a new list ⇒ it gets a **new slot** and the old handle is invalidated.
    pub fn remove_current_as_list(&mut self) -> Option<List<T, S>>
    where
        S: Default,
    {
        let value = self.remove_current()?;

        let mut out = List::default();

        out.push_back(value);

        Some(out)
    }

    /// Mirrors `CursorMut::splice_before`: attaches the elements of `list` **in their
    /// original order** before the current element (ghost position = append at the back).
    ///
    /// Differs from std: std moves the chain in `O(1)` (and requires a shared allocator),
    /// whereas here each element is `insert_before`d, `O(list.len())`, and gets a **new slot in this list** (old handles invalidated).
    pub fn splice_before(&mut self, list: List<T, S>) {
        for value in list {
            self.insert_before(value);
        }
    }

    /// Mirrors `CursorMut::splice_after`: attaches after the current element, **order preserved**.
    ///
    /// (`insert_after` always inserts right next to the cursor, so feeding elements in reverse yields `list` appended as-is.)
    pub fn splice_after(&mut self, list: List<T, S>) {
        for value in list.into_iter().rev() {
            self.insert_after(value);
        }
    }

    /// Mirrors `CursorMut::split_before`: detaches the part **before** the current element
    /// into a new list; the cursor stays on the head of the remainder (the original current element becomes the head). Ghost position (the back) ⇒ the whole list is detached.
    ///
    /// **Different complexity**: std is `O(1)` (pointer split), ours is `O(pos)` — slots live in one contiguous storage, so detaching half requires moving
    /// elements; **moved elements get slots in the new list and old handles are invalidated**.
    pub fn split_before(&mut self) -> List<T, S>
    where
        S: Default,
    {
        let at = self.index().unwrap_or(self.list.len);

        let mut out = List::default();

        for _ in 0..at {
            // `at <= len` is guaranteed by `index()`
            let value = self.list.pop_front().expect("at <= len");

            out.push_back(value);
        }

        if self.slot == NIL {
            self.pos = 0;
        } else if self.pos != POS_UNKNOWN {
            // The current element is now the chain head
            self.pos = 0;
        }

        out
    }

    /// Mirrors `CursorMut::split_after`: detaches the part **after** the current
    /// element into a new list (the cursor and the remainder stay put). Complexity is likewise `O(len - pos)`, not std's `O(1)`.
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

    /// Mirrors `CursorMut::remove_current`: returns the removed element and moves the
    /// cursor to the next one (to the ghost position if the tail was removed). Returns `None` at the ghost position.
    pub fn remove_current(&mut self) -> Option<T> {
        if self.slot == NIL {
            return None;
        }

        let slot = self.slot;
        let next = self.list.storage.next(slot);
        // Check before detaching: `tail` changes once the chain tail is removed
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

    /// Mirrors `CursorMut::push_front`. The node the cursor points at is unchanged.
    pub fn push_front(&mut self, item: T) {
        self.list.push_front(item);

        if self.slot != NIL && self.pos != POS_UNKNOWN {
            self.pos += 1;
        }
    }

    /// Mirrors `CursorMut::push_back`. The node the cursor points at is unchanged.
    pub fn push_back(&mut self, item: T) {
        self.list.push_back(item);
    }

    /// Mirrors `CursorMut::pop_front`. If the cursor pointed at the front, it moves to the new front.
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

    /// Mirrors `CursorMut::pop_back`. If the cursor pointed at the back, it moves to the ghost position.
    pub fn pop_back(&mut self) -> Option<T> {
        let back = self.list.tail;
        let value = self.list.pop_back()?;

        if self.slot != NIL && self.slot == back {
            self.slot = NIL;
            self.pos = 0;
        }

        Some(value)
    }

    /// Mirrors `CursorMut::front`.
    pub fn front(&self) -> Option<&T> {
        self.list.front()
    }

    /// Mirrors `CursorMut::front_mut`.
    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.list.front_mut()
    }

    /// Mirrors `CursorMut::back`.
    pub fn back(&self) -> Option<&T> {
        self.list.back()
    }

    /// Mirrors `CursorMut::back_mut`.
    pub fn back_mut(&mut self) -> Option<&mut T> {
        self.list.back_mut()
    }

    /// Mirrors `CursorMut::as_cursor`.
    pub fn as_cursor(&self) -> Cursor<'_, T, S> {
        Cursor::at(self.list, self.slot, self.pos)
    }

    /// Mirrors `CursorMut::as_list`.
    pub fn as_list(&self) -> &List<T, S> {
        self.list
    }

    // --------------------------------------------------------
    // Library extensions (absent from std)
    // --------------------------------------------------------

    /// **Library extension**: move to the `pos`-th element; out of bounds returns
    /// `false` and leaves the cursor unmoved. `O(min(pos, len - 1 - pos))`.
    pub fn seek(&mut self, pos: usize) -> bool {
        if pos >= self.list.len {
            return false;
        }

        self.slot = self.list.slot_at(pos);
        self.pos = pos;

        true
    }

    /// **Library extension**: move `offset` steps relative; out of bounds returns
    /// `false` and leaves the cursor unmoved. `O(|offset|)`; but if the cursor was
    /// **entered via [`Slot`](crate::Slot)** (position unknown), it first walks the chain to find the current position: `O(len + |offset|)`.
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

    /// **Library extension**: whether it points at the front element. `O(1)`.
    pub fn is_head(&self) -> bool {
        self.slot != NIL && self.slot == self.list.head
    }

    /// **Library extension**: whether it points at the back element. `O(1)`.
    pub fn is_tail(&self) -> bool {
        self.slot != NIL && self.slot == self.list.tail
    }
}
