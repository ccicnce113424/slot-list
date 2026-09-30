//! Stable handles: an identity for a slot in contiguous memory.

/// Slot handle: an identity that is **stable while the element lives**, and can be
/// stored in other containers (hash maps, adjacency lists, undo stacks, ...).
///
/// It points at a **physical slot** (`slot`), not a **logical position** (`pos`); the
/// two have no conversion formula; see the "two positions" section of the crate docs.
/// Getting a handle: [`Cursor::slot`](crate::Cursor::slot) /
/// [`CursorMut::slot`](crate::CursorMut::slot) / [`List::front_slot`](crate::List::front_slot).
/// Using a handle: [`List::cursor_at`](crate::List::cursor_at) / [`List::remove_slot`](crate::List::remove_slot)
/// / [`List::move_to_front`](crate::List::move_to_front); all `O(1)`.
///
/// Lifetime: `remove` / `pop_*` / `clear` push the slot back onto the free-list, after
/// which the old `Slot` is **no longer valid** (`cursor_at` / `remove_slot` return
/// `None`). There is **no generation check**, though: if that slot is reused by a later
/// insert, the old `Slot` points at the **new element** (classic ABA). To have "a stale
/// handle is detectable", use a generation-tagged implementation such as `slotmap` /
/// `fast-list`.
///
/// Validity is **O(1) checkable**: the slot's free bit lives in the top bit of `prev`,
/// and the check does not touch `data` (a free slot's `data` is uninitialized; touching
/// it is UB).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub(crate) usize);

impl Slot {
    /// Internal slot index (for debugging/serialization; normally treat the handle
    /// as opaque).
    pub const fn to_usize(self) -> usize {
        self.0
    }
}
