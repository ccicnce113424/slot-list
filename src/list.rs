use core::fmt;
use core::marker::PhantomData;

use crate::cursor::{Cursor, CursorMut};
use crate::iter::{IntoIter, Iter, IterMut};
use crate::slot::Slot;
use crate::storage::{NIL, Nodes, PackedLinks, Split, Storage};

/// Doubly linked list: `Vec` indices + a free list; the memory layout is chosen by `S`.
///
/// See the three aliases in the crate docs: [`SplitList`](crate::SplitList) /
/// [`PackedLinksList`](crate::PackedLinksList) / [`NodesList`](crate::NodesList).
///
/// # No sentinel values in the arrays
///
/// Every value in the `prev` / `next` arrays is a **valid slot index**. The ends of both
/// chains are "dummy" links: the head node's `prev` and the tail node's `next` are never
/// read, but they are still valid indices (a fresh slot is initialized to a self-loop and
/// may later keep a stale but valid index). `NIL` is used only as a scalar argument /
/// cursor tag; it is **never written into an array**.
///
/// This is what [`append`](List::append) rests on: a whole relocated run of indices can be
/// `+= base` unconditionally, with no per-element "is this a sentinel?" test.
///
/// # Empty state of the fields
///
/// **All four endpoint fields reset to `NIL` when their chain is empty**: `head` / `tail`
/// when `len == 0`, `free_head` / `free_tail` when the free chain empties. So "is it empty?"
/// is one field load plus a comparison, instead of `storage.slots() - len` (two loads plus a
/// subtraction) — the latter runs on every free/alloc and is measurably slower on the `churn`
/// benchmark. The reset itself is cheap: it writes two fields only on the transition to empty.
///
/// # Shape of the chains
///
/// Walking a chain in one direction must be **count-driven**, not "walk until NIL" (the dummy
/// links at the ends are not NIL). [`Iter`] / [`IterMut`] already count down `remaining`;
/// `retain` / `clear` also finish by `len`.
pub struct List<T, S: Storage<T>> {
    /// Head of the live chain; [`NIL`] when empty.
    pub(crate) head: usize,
    /// Tail of the live chain; [`NIL`] when empty.
    pub(crate) tail: usize,
    /// Head of the free chain; meaningless when the free count is 0.
    pub(crate) free_head: usize,
    /// Tail of the free chain; meaningless when the free count is 0. Updated only on the empty→non-empty transition.
    pub(crate) free_tail: usize,
    /// Number of live elements.
    pub(crate) len: usize,
    pub(crate) storage: S,
    /// Exists only to "use" `T`: `List` is defined for **any** `S: Storage<T>`, and `T`
    /// appears only in that bound — Rust **does not count occurrences in where-clauses as a
    /// use**, so removing this field gives `error[E0392]: type parameter `T` is never used`.
    /// (Concrete aliases like `SplitList<T> = List<T, Split<T>>` are unaffected, since there `T`
    /// reaches a field type through `Split<T>`; what is affected is the generic definition itself.)
    ///
    /// `PhantomData<T>` is chosen over any other marker because its meaning happens to match
    /// reality exactly:
    ///
    /// - **Owns `T`**: dropping a `List<T, S>` does drop `T` ([`Drop`](List#impl-Drop) walks the
    ///   live chain calling `assume_init_drop`), and dropck needs to know that;
    /// - **Covariant in `T`**: matching the three `Storage` types (`Vec<MaybeUninit<T>>` /
    ///   `Link<T>` / `Node<T>`) ⇒ `SplitList<&'static str>` can be used as `SplitList<&'a str>`;
    /// - **auto traits follow `T`**: `List<T, S>: Send` iff `T: Send` (same for `Sync`).
    ///
    /// The last two are covered by a compile-time test (`tests::auto_traits_and_variance`).
    marker: PhantomData<T>,
}

impl<T, I: crate::Ix> List<T, Split<T, I>> {
    /// Empty list (Split layout; the index width comes from `Split`'s second parameter).
    pub const fn new() -> Self {
        Self {
            storage: Split::new(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            free_tail: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T, I: crate::Ix> List<T, PackedLinks<T, I>> {
    /// Empty list (PackedLinks layout; the index width comes from `PackedLinks`'s second parameter).
    pub const fn new() -> Self {
        Self {
            storage: PackedLinks::new(),
            head: NIL,
            tail: NIL,
            free_head: NIL,
            free_tail: NIL,
            len: 0,
            marker: PhantomData,
        }
    }
}

impl<T, I: crate::Ix> List<T, Nodes<T, I>> {
    /// Empty list (Nodes layout; the index width comes from `Nodes`'s second parameter).
    pub const fn new() -> Self {
        Self {
            storage: Nodes::new(),
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
    /// **Library extension**: create an empty list after reserving capacity for `capacity` slots.
    ///
    /// Slots live in **contiguous memory**, so reserving up front avoids repeated reallocation
    /// during construction (especially for the Split layout: one reserve replaces three growth
    /// steps across the three arrays).
    pub fn with_capacity(capacity: usize) -> Self {
        let mut list = Self::default();

        list.reserve(capacity);

        list
    }
}

impl<T, S: Storage<T>> List<T, S> {
    // --------------------------------------------------------
    // Queries
    // --------------------------------------------------------

    /// Number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Total number of allocated slots (live + free). `len() <= capacity()`; the difference is
    /// the free slot count.
    ///
    /// Like `Vec::capacity`, this is "how many slots fit without reallocating", but the unit is
    /// **slots**, not elements: slots are reused through the free chain, so `capacity()` does not
    /// change across `push`/`pop`.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.storage.slots()
    }

    /// Return the **excess capacity** of the underlying `Vec` to the allocator
    /// (`Vec::shrink_to_fit`).
    ///
    /// **Not** the semantics of `Vec::shrink_to_fit`: that shrinks to `len()`, while here the
    /// **slot count is unchanged** — free slots are part of the free chain and are what
    /// [`Slot`](crate::Slot) handles point at; discarding them would invalidate handles and
    /// break the chains. So this only returns capacity that was allocated but never used.
    pub fn shrink_to_fit(&mut self) {
        self.storage.shrink_to_fit();
    }

    /// Whether the list is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// **Library extension** (std's linked list has no capacity concept): reserve capacity for `additional` slots.
    pub fn reserve(&mut self, additional: usize) {
        self.storage.reserve(additional);
    }

    // --------------------------------------------------------
    // Internals: node operations
    // --------------------------------------------------------

    /// Insert `pos` between `prev` and `next` (both are real neighbors).
    #[inline]
    pub(crate) fn insert_between(&mut self, pos: usize, prev: usize, next: usize) {
        self.storage.set_next(prev, pos);
        self.storage.set_prev(next, pos);
        self.storage.set_prev(pos, prev);
        self.storage.set_next(pos, next);
    }

    /// Insert `pos` as the head. The caller has already counted `pos` in `len` ([`alloc_slot`](Self::alloc_slot) did).
    #[inline]
    pub(crate) fn insert_front(&mut self, pos: usize) {
        if self.len == 1 {
            // Was empty: both links are dummy (self-loops)
            self.storage.set_prev(pos, pos);
            self.storage.set_next(pos, pos);
            self.head = pos;
            self.tail = pos;
        } else {
            let head = self.head;

            self.storage.set_prev(head, pos);
            self.storage.set_next(pos, head);
            // The head's `prev` is a dummy link (nobody reads where it points), but it
            // **must** be written: it also holds the "slot is live" mark bit, and leaving the
            // old free-chain value would make this new head look free.
            self.storage.set_prev(pos, pos);
            self.head = pos;
        }
    }

    /// Insert `pos` as the tail.
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

    /// Detach a slot from the head of the free chain. The free chain must be non-empty on entry.
    #[inline]
    fn pop_free(&mut self) -> usize {
        let slot = self.free_head;
        let next = self.storage.next(slot);

        if next == slot {
            // Only one left (self-loop terminates) ⇒ the chain is empty: reset
            self.free_head = NIL;
            self.free_tail = NIL;
        } else {
            self.free_head = next;
        }

        slot
    }

    /// Push a slot back onto the head of the free chain.
    #[inline]
    pub(crate) fn push_free(&mut self, slot: usize) {
        // The one place that must explicitly set the mark bit: from here on it is no longer a live slot
        self.storage.mark_free(slot);

        if self.free_head == NIL {
            // Was an empty chain: it is also the tail, so write a self-loop into `next` (NIL must never enter an array)
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
            // Empty list: reset the endpoints (readers of `head`/`tail` can keep the old shape)
            self.head = NIL;
            self.tail = NIL;
        }

        value
    }

    /// Detach a live slot from the chain (**does not touch `len` or recycle it**). Either end may
    /// be the head / tail, so both are checked. Shared by element moves
    /// ([`move_to_front`](Self::move_to_front)) and `unlink_slot`.
    #[inline]
    fn detach(&mut self, slot: usize) {
        let prev = self.storage.prev(slot);
        let next = self.storage.next(slot);

        if slot == self.head {
            // Reset to NIL by `free_slot` when the list becomes empty
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

    /// Detach a live node and recycle it.
    #[inline]
    pub(crate) fn unlink_slot(&mut self, slot: usize) -> T {
        self.detach(slot);
        self.free_slot(slot)
    }

    /// Handle validation: in range **and** not a free slot. **Does not touch `data`** (a free slot's `data` is uninitialized).
    #[inline]
    fn live_slot(&self, slot: Slot) -> Option<usize> {
        let slot = slot.0;

        (slot < self.storage.slots() && !self.storage.is_free(slot)).then_some(slot)
    }

    /// Walk `steps` nodes forward along the chain. The caller guarantees `steps < len`.
    #[inline]
    fn move_forward(&self, slot: usize, steps: usize) -> usize {
        let mut current = slot;

        for _ in 0..steps {
            current = self.storage.next(current);
        }

        current
    }

    /// Walk `steps` nodes backward along the chain. The caller guarantees `steps < len`.
    #[inline]
    fn move_backward(&self, slot: usize, steps: usize) -> usize {
        let mut current = slot;

        for _ in 0..steps {
            current = self.storage.prev(current);
        }

        current
    }

    /// Index of the `pos`-th live node; `pos` must be `< len`.
    /// Starts from the **nearer end**: `pos` steps forward, or `len - 1 - pos` steps backward.
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
    // End operations
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

    /// Mirrors `LinkedList::push_front`.
    #[inline]
    pub fn push_front(&mut self, value: T) {
        let _ = self.push_front_slot(value);
    }

    /// Mirrors `LinkedList::push_back`.
    #[inline]
    pub fn push_back(&mut self, value: T) {
        let _ = self.push_back_slot(value);
    }

    /// Mirrors `LinkedList::push_front_mut`: insert and return a reference to the new element.
    #[inline]
    pub fn push_front_mut(&mut self, value: T) -> &mut T {
        let slot = self.push_front_slot(value);

        unsafe { self.storage.data_mut(slot).assume_init_mut() }
    }

    /// Mirrors `LinkedList::push_back_mut`: insert and return a reference to the new element.
    #[inline]
    pub fn push_back_mut(&mut self, value: T) -> &mut T {
        let slot = self.push_back_slot(value);

        unsafe { self.storage.data_mut(slot).assume_init_mut() }
    }

    /// Mirrors `LinkedList::pop_front`.
    #[inline]
    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }

        let slot = self.head;

        // The head node's `prev` is a dummy link, so there is nothing to update; an emptied list gets reset
        self.head = self.storage.next(slot);

        Some(self.free_slot(slot))
    }

    /// Mirrors `LinkedList::pop_back`.
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
    // Endpoint access
    // --------------------------------------------------------

    /// Mirrors `LinkedList::front`.
    #[inline]
    pub fn front(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data(self.head).assume_init_ref() })
    }

    /// Mirrors `LinkedList::back`.
    #[inline]
    pub fn back(&self) -> Option<&T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data(self.tail).assume_init_ref() })
    }

    /// Mirrors `LinkedList::front_mut`.
    #[inline]
    pub fn front_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data_mut(self.head).assume_init_mut() })
    }

    /// Mirrors `LinkedList::back_mut`.
    #[inline]
    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            return None;
        }

        Some(unsafe { self.storage.data_mut(self.tail).assume_init_mut() })
    }

    // --------------------------------------------------------
    // Cursors
    // --------------------------------------------------------

    /// Mirrors `LinkedList::cursor_front`.
    #[inline]
    pub fn cursor_front(&self) -> Cursor<'_, T, S> {
        Cursor::at(self, self.head, 0)
    }

    /// Mirrors `LinkedList::cursor_front_mut`.
    #[inline]
    pub fn cursor_front_mut(&mut self) -> CursorMut<'_, T, S> {
        CursorMut::at(self, self.head, 0)
    }

    /// Mirrors `LinkedList::cursor_back`.
    #[inline]
    pub fn cursor_back(&self) -> Cursor<'_, T, S> {
        Cursor::at(self, self.tail, self.len.saturating_sub(1))
    }

    /// Mirrors `LinkedList::cursor_back_mut`.
    #[inline]
    pub fn cursor_back_mut(&mut self) -> CursorMut<'_, T, S> {
        CursorMut::at(self, self.tail, self.len.saturating_sub(1))
    }

    /// **Library extension**: get a cursor by stable handle, **O(1)**. Returns `None` for a stale
    /// handle (slot freed / out of range).
    ///
    /// A cursor entered this way **does not know its logical position**: `index()` / `move_steps`
    /// walk the chain to compute it on demand (`O(len)`), while every handle-based
    /// insert/remove/move stays `O(1)`.
    ///
    /// There is no generation check: once a slot is reused, an old handle points at the **new
    /// element** (see the [`Slot`] docs).
    pub fn cursor_at(&self, slot: Slot) -> Option<Cursor<'_, T, S>> {
        self.live_slot(slot)
            .map(|slot| Cursor::at(self, slot, crate::cursor::POS_UNKNOWN))
    }

    /// **Library extension**: get a **mutable** cursor by stable handle, **O(1)**. Same semantics as [`cursor_at`](Self::cursor_at).
    pub fn cursor_at_mut(&mut self, slot: Slot) -> Option<CursorMut<'_, T, S>> {
        self.live_slot(slot)
            .map(|slot| CursorMut::at(self, slot, crate::cursor::POS_UNKNOWN))
    }

    /// **Library extension**: remove and return the element by handle, **O(1)** (no logical position needed). Returns `None` for a stale handle.
    pub fn remove_slot(&mut self, slot: Slot) -> Option<T> {
        self.live_slot(slot).map(|slot| self.unlink_slot(slot))
    }

    /// **Library extension**: handle of the head element, `O(1)`.
    pub fn front_slot(&self) -> Option<Slot> {
        (self.head != NIL).then_some(Slot(self.head))
    }

    /// **Library extension**: handle of the tail element, `O(1)`.
    pub fn back_slot(&self) -> Option<Slot> {
        (self.tail != NIL).then_some(Slot(self.tail))
    }

    /// **Library extension**: move the element referenced by the handle to the head, **O(1)** (one
    /// of the LRU primitives). Returns `None` for a stale handle; a no-op if it is already the head.
    pub fn move_to_front(&mut self, slot: Slot) -> Option<()> {
        let slot = self.live_slot(slot)?;

        if slot != self.head {
            self.detach(slot);
            self.insert_front(slot);
        }

        Some(())
    }

    /// **Library extension**: move the element referenced by the handle to the tail, **O(1)**. Same semantics as [`move_to_front`](Self::move_to_front).
    pub fn move_to_back(&mut self, slot: Slot) -> Option<()> {
        let slot = self.live_slot(slot)?;

        if slot != self.tail {
            self.detach(slot);
            self.insert_back(slot);
        }

        Some(())
    }

    /// **Library extension**: logical position of a handle, **O(len)** (it must be counted).
    /// Returns `None` for a stale handle. Fine for the occasional "which index is this?"; if you
    /// ask often, take a cursor with [`cursor_at`](Self::cursor_at).
    pub fn pos_of(&self, slot: Slot) -> Option<usize> {
        self.live_slot(slot)
            .map(|slot| crate::cursor::pos_of_slot(self, slot))
    }

    /// Walk the **free chain** (`free_head` → `free_tail`, **LIFO order**: most recently recycled
    /// first).
    ///
    /// Together with [`iter_slots`](Self::iter_slots) this exposes the entire state of the list
    /// (values + links + free order) for your own serialization / persistence / shared memory.
    /// Restore with [`into_raw`](Self::into_raw) and each layout's `from_parts`.
    pub fn free_slots(&self) -> impl Iterator<Item = Slot> + '_ {
        let mut remaining = self.storage.slots() - self.len;
        let mut slot = self.free_head;

        core::iter::from_fn(move || {
            if remaining == 0 || slot == NIL {
                return None;
            }

            remaining -= 1;

            let current = slot;
            let next = self.storage.next(slot);

            // Self-loop = tail of the free chain (a single-element chain is also a self-loop)
            slot = if next == current { NIL } else { next };

            Some(Slot(current))
        })
    }

    /// **Library extension**: yield each element's stable handle while iterating, `(Slot, &T)`.
    ///
    /// Use it to join "list order" with "your own hash map": store the handle as a key and later
    /// go back to that element (`O(1)`) with [`cursor_at`](Self::cursor_at) /
    /// [`remove_slot`](Self::remove_slot).
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

    /// **Library extension** (std has no random access): locate the `pos`-th element.
    /// Starts from the nearer end, O(min(pos, len - 1 - pos)).
    #[inline]
    pub fn at(&mut self, pos: usize) -> Option<CursorMut<'_, T, S>> {
        if pos >= self.len {
            return None;
        }

        let slot = self.slot_at(pos);

        Some(CursorMut::at(self, slot, pos))
    }

    // --------------------------------------------------------
    // Iteration
    // --------------------------------------------------------

    /// Mirrors `LinkedList::iter`.
    #[inline]
    pub fn iter(&self) -> Iter<'_, T, S> {
        Iter::new(self)
    }

    /// Mirrors `LinkedList::iter_mut`.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, T, S> {
        IterMut::new(self)
    }

    // --------------------------------------------------------
    // Bulk operations
    // --------------------------------------------------------

    /// Mirrors `LinkedList::contains`. O(N).
    #[inline]
    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|item| item == value)
    }

    /// Mirrors `LinkedList::retain`. O(N).
    ///
    /// The loop terminates by **fixed count** (capture `len` before the loop and decrement), not
    /// by testing `len > 0` + `index == tail` each round — the latter costs an extra comparison
    /// per round and is measurably slower at 1M elements. The dummy links at the chain ends are
    /// not NIL, so traversal must be count-driven.
    #[inline]
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut T) -> bool,
    {
        let mut slot = self.head;
        // Count-driven: the dummy links at both ends are not NIL, so the chain has no "natural end"
        let mut remaining = self.len;

        while remaining > 0 {
            remaining -= 1;

            // Fetch the next slot first: unlinking the current slot rewrites its own links
            let next = self.storage.next(slot);

            let keep = f(unsafe { self.storage.data_mut(slot).assume_init_mut() });

            if !keep {
                let _ = self.unlink_slot(slot);
            }

            slot = next;
        }
    }

    /// Mirrors `LinkedList::append`: move **all slots** of `other` to the end in one block — the
    /// slot indices are shifted in bulk as the block moves (indices written once), and each chain
    /// is spliced once. Complexity O(total slots in `other`), independent of the number of live
    /// elements there.
    ///
    /// Semantics: **`other`'s free slots move along too**; afterwards `other` is "an empty list
    /// whose capacity stays with it, with its slots relocated". Returns immediately if `other`
    /// has zero slots.
    ///
    /// For "move element by element to the end, preferring to reuse your own free slots first"
    /// (cost proportional to live elements), use [`append_elementwise`](List::append_elementwise);
    /// the trade-off is documented there.
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

        // 1) Move the slots: data in one block, rebasing both chains' indices by `base` as they move
        self.storage.append(&mut other.storage);

        // 2) Live chain: splice all of `other`'s chain behind our tail
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

        // 3) Free chain: link our tail to `other`'s head (our entry point is unchanged, preserving LIFO)
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

        // `other` was emptied: reset all four endpoint fields (invariant: a field is NIL ⟺ its chain is empty)
        other.len = 0;
        other.head = NIL;
        other.tail = NIL;
        other.free_head = NIL;
        other.free_tail = NIL;
    }

    /// **Library extension**: move all elements of `other` one by one to the end of `self`
    /// (`other` becomes empty).
    ///
    /// Equivalent to `while let Some(value) = other.pop_front() { self.push_back(value); }`: it
    /// prefers filling **existing free slots** (already allocated and touched, no new pages) and
    /// only grows when they run out. The cost is proportional to the **number of live elements**,
    /// independent of `other`'s slot count.
    ///
    /// Trade-off against [`append`](List::append): use this one when your own free slots suffice
    /// or when `other` is sparse; otherwise `append` wins.
    pub fn append_elementwise(&mut self, other: &mut Self) {
        while let Some(value) = other.pop_front() {
            self.push_back(value);
        }
    }

    /// Mirrors `LinkedList::split_off`. Panics if `at > len`. O(N).
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

    /// Mirrors `LinkedList::remove`. Panics if `at >= len`. O(N).
    pub fn remove(&mut self, at: usize) -> T {
        assert!(at < self.len, "remove slot out of bounds");

        let slot = self.slot_at(at);

        self.unlink_slot(slot)
    }

    /// Extract the **relocatable state** of the whole chain (dropping no element).
    ///
    /// This is the public entry point for the "relocatable, pointer-free state" promise of
    /// `PERFORMANCE.md` §8: the state is the slot array ([`Split::into_parts`](crate::Split::into_parts)
    /// and friends) plus five numbers ([`RawList`]'s accessors), with **no pointers at all**, so it
    /// can be serialized / placed in shared memory / `mmap`ped with **no pointer fixup**.
    ///
    /// ```
    /// # use slot_list::SplitList;
    /// let mut list: SplitList<u32> = SplitList::new();
    /// list.extend([1, 2, 3]);
    /// let removed = list.pop_front().unwrap();
    /// assert_eq!(removed, 1);          // leaves one free slot, so the free chain is non-empty
    ///
    /// let raw = list.into_raw();
    /// // Now you can: write raw.storage().as_parts() to disk, record raw.head_slot()/free_slots as metadata
    /// assert_eq!(raw.len(), 2);
    /// assert_eq!(raw.capacity(), 3);
    ///
    /// let list = unsafe { SplitList::from_raw(raw) };   // put it back as-is
    /// assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![2, 3]);
    /// ```
    pub fn into_raw(self) -> RawList<T, S> {
        RawList {
            inner: core::mem::ManuallyDrop::new(self),
        }
    }

    /// Put back the state extracted by [`into_raw`](Self::into_raw).
    ///
    /// # Safety
    ///
    /// `raw` must describe a **self-consistent** chain: links are valid indices, free slots carry
    /// the free mark bit, `len` equals the live slot count, and `head`/`tail`/`free_head`/`free_tail`
    /// match reality (either coming from [`into_raw`](Self::into_raw) or rebuilt by the same rules).
    /// A live slot's `data` must be initialized. Violating this is UB, not a panic.
    pub unsafe fn from_raw(raw: RawList<T, S>) -> Self {
        // `RawList` has a `Drop` (frees only the array), so freeze it first and then bit-wise take out the inner value
        let raw = core::mem::ManuallyDrop::new(raw);
        let inner = unsafe { core::ptr::read(&raw.inner) };

        core::mem::ManuallyDrop::into_inner(inner)
    }

    /// Mirrors `LinkedList::clear`. Keeps the allocated capacity and slots.
    ///
    /// Done in a single pass: drop each live element in turn and push its slot back onto the free
    /// chain in place. The loop condition uses `len` (the tail's dummy link is not NIL, so "walk
    /// until NIL" does not work).
    ///
    /// Why not "`pop_front` until empty": `pop_front` routes every element through
    /// `unlink_slot` / `free_slot` — read `prev`, patch the neighbors, check
    /// endpoints — and `assume_init_read()` **moves `T` out of the slot** before dropping it;
    /// `clear` only reads `next`, drops **in place**, and resets the endpoints once.
    ///
    /// # Hybrid: density ≥ 0.5 scans slots sequentially, otherwise walks the chain
    ///
    /// The two paths have completely different cost models: **walking the live chain is O(live)
    /// random slot accesses** (each a dependent load plus a random read-modify-write), while
    /// **scanning is O(slots) sequential accesses** (streaming over just the few mark bytes). The
    /// break-even density moves with **index width** and **layout**: a full table favors scanning
    /// by up to ~7× with a `u32` Split index, while a sparse table favors the walk by orders of
    /// magnitude (at density 0.001 the walk is ~100× faster). The threshold is therefore **0.5**:
    /// scan when `2 * len >= slots`. Across the six layout/index combinations this keeps the
    /// result between 0.99× and 1.43× (worst case a tie, no combination measurably slower).
    ///
    /// Data comes from `probe_clear_vs_scan` in `src/tests.rs`:
    /// `cargo test --release -- --ignored --nocapture probe_clear_vs_scan`.
    ///
    /// **Reading note**: the scan branch is purely bandwidth-bound and can swing 3~4× with other
    /// load on the machine, whereas the walk is latency-bound and stable ⇒ the ratio must be
    /// taken **within a single run** (and ideally on an idle machine). `probe_clear_vs_scan`
    /// already produces both columns in the same run, so do not divide numbers from different
    /// moments.
    ///
    /// Known boundary cost: a full-table scan with Drop glue merely ties, because the drops
    /// themselves consume the bandwidth; deliberately not testing `needs_drop` separately keeps
    /// the decision to a single predicate (measured worst case ~5%, within noise).
    ///
    /// Complexity: the scan is O(slots) but is only chosen when `len >= slots/2`, so the worst
    /// case is O(2·live); a just-emptied deque still takes the walk's microsecond path.
    #[inline]
    pub fn clear(&mut self) {
        let slots = self.storage.slots();

        // Density ≥ 0.5 ⇒ scan slots sequentially. `slots == 0` also lands here and is a no-op.
        if self.len * 2 >= slots {
            for slot in 0..slots {
                // A slot already on the free chain must **not** be pushed again: `push_free` would
                // point its `next` at the chain head while its old position still points at it
                // ⇒ a cycle, and the free chain could never be drained.
                if self.storage.is_free(slot) {
                    continue;
                }

                unsafe { self.storage.data_mut(slot).assume_init_drop() };

                self.push_free(slot);
            }

            self.head = NIL;
            self.tail = NIL;
            self.len = 0;

            return;
        }

        // Sparse: walk the live chain, touching only live slots
        while self.len > 0 {
            let slot = self.head;

            // Fetch the next slot first: `push_free` rewrites the current slot's `next`
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
// std-style trait implementations
// ============================================================

/// The **relocatable state** of the whole chain: the slot array plus five numbers
/// (`head`/`tail`/`free_head`/`free_tail`/`len`).
///
/// Extracted by [`List::into_raw`] and restored by [`List::from_raw`]. It does **not** own the
/// values: dropping a `RawList` frees only the slot array and **never drops `T`** (as with
/// `Vec::into_raw_parts`, the elements live in `MaybeUninit` and are not dropped by the storage).
/// To get the elements back, restore them into a `List` first.
///
/// The accessors expose everything serialization needs: the array from `storage()`
/// ([`Split::as_parts`](crate::Split::as_parts) / [`PackedLinks::as_parts`](crate::PackedLinks::as_parts) /
/// [`Nodes::as_parts`](crate::Nodes::as_parts)), and the five numbers from here.
pub struct RawList<T, S: Storage<T>> {
    inner: core::mem::ManuallyDrop<List<T, S>>,
}

impl<T, S: Storage<T>> RawList<T, S> {
    /// Number of live elements (same as `List::len()` at extraction time).
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.len
    }

    /// Whether there are no live elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total number of slots (live + free).
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.storage.slots()
    }

    /// The slot array (ownership of the `Vec` stays with `RawList`).
    #[inline]
    pub fn storage(&self) -> &S {
        &self.inner.storage
    }

    /// The slot array (writable) — use it to modify in place (e.g. compact before serializing).
    ///
    /// **Breaking an invariant means UB when the `RawList` is later put back into a `List`**
    /// (links must be valid indices, free slots must carry the mark bit).
    #[inline]
    pub fn storage_mut(&mut self) -> &mut S {
        &mut self.inner.storage
    }

    /// Head slot; `None` when empty.
    #[inline]
    pub fn head_slot(&self) -> Option<Slot> {
        (self.inner.head != NIL).then_some(Slot(self.inner.head))
    }

    /// Tail slot; `None` when empty.
    #[inline]
    pub fn tail_slot(&self) -> Option<Slot> {
        (self.inner.tail != NIL).then_some(Slot(self.inner.tail))
    }

    /// Head of the free chain (the most recently recycled slot).
    #[inline]
    pub fn free_head_slot(&self) -> Option<Slot> {
        (self.inner.free_head != NIL).then_some(Slot(self.inner.free_head))
    }

    /// Tail of the free chain (the earliest recycled slot not yet reused).
    #[inline]
    pub fn free_tail_slot(&self) -> Option<Slot> {
        (self.inner.free_tail != NIL).then_some(Slot(self.inner.free_tail))
    }

    /// Rebuild from the five literals (for deserialization).
    ///
    /// # Safety
    ///
    /// Same as [`List::from_raw`]: links, mark bits, `len` and `head`/`tail` must be self-consistent.
    pub unsafe fn from_fields(
        storage: S,
        head: Option<Slot>,
        tail: Option<Slot>,
        free_head: Option<Slot>,
        free_tail: Option<Slot>,
        len: usize,
    ) -> Self {
        RawList {
            inner: core::mem::ManuallyDrop::new(List {
                storage,
                head: head.map_or(NIL, |slot| slot.0),
                tail: tail.map_or(NIL, |slot| slot.0),
                free_head: free_head.map_or(NIL, |slot| slot.0),
                free_tail: free_tail.map_or(NIL, |slot| slot.0),
                len,
                marker: PhantomData,
            }),
        }
    }
}

impl<T, S: Storage<T>> Drop for RawList<T, S> {
    fn drop(&mut self) {
        // Frees only the slot array. The elements are `MaybeUninit<T>`, and `S` never drops them
        // ⇒ this neither loses the elements' value semantics nor leaks the array's memory.
        let storage = unsafe { core::ptr::read(&self.inner.storage) };

        drop(storage);
    }
}

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

/// Drop: walks only the live chain and drops **in place**, never touching the free chain.
///
/// Why not [`clear`](List::clear): `clear` also has to push every slot back onto the free chain
/// (`mark_free` + `set_next` + endpoint reset), while here the whole storage is about to be freed
/// anyway, so that maintenance is pure loss. The win grows with `T` (it removes a per-element
/// value move) and reaches several-fold for small payloads; the numbers are in `PERFORMANCE.md`.
///
/// This also covers dropping an `into_iter()` halfway ([`IntoIter`] is a thin wrapper over
/// `pop_front`, and the remainder is finished off here).
///
/// Counts by `len` rather than "walk until NIL": the tail's `next` is a dummy link (a self-loop,
/// or leftover from its free period).
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
