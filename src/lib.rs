#![cfg_attr(not(feature = "std"), no_std)]
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

//! A doubly linked list whose nodes all live in a **preallocated contiguous
//! region**: **the slot index is the element's identity** (`slot`, constant for
//! the element's lifetime), and deleted slots are reused through a free-list
//! (LIFO). That is where the name **`slot-list`** comes from.
//!
//! # Two "positions": logical position `pos` and physical slot `slot`
//!
//! The code and the docs keep these two terms strictly apart; **there is no
//! conversion formula between them**:
//!
//! | | logical position `pos` | physical slot `slot` |
//! |---|---|---|
//! | Meaning | the element's order in the chain (which one) | the element's index in contiguous memory (= identity) |
//! | Range | `0 .. len()` | `0 .. storage.slots()` |
//! | Stability | changes on every insert/remove | **constant while the element lives** (usable as a handle) |
//! | How to get it | walk the chain from an endpoint; `Cursor::index()` | **handle**: `Cursor::slot` / `CursorMut::slot` /
//!   `List::front_slot` / `back_slot` / `iter_slots` (all `O(1)`); the type is [`Slot`] (a raw index,
//!   8 B, **no generation tag** ⇒ a stale handle is undetectable; for the trade-off vs `fast-list` see `PERFORMANCE.md` §9) |
//! | Conversion | `slot_at(pos)`: walk the chain, `O(min(pos, len-1-pos))` | `pos_of(slot)`: walk the chain, `O(len)` |
//!
//! Any "position" in the public API means the **logical position**: [`List::at`] /
//! [`List::remove`] / [`Cursor::index`] (matching std's `Cursor::index`: returns the
//! logical position, `None` for the ghost position) / [`CursorMut::seek`] /
//! [`CursorMut::move_steps`].
//!
//! **Slots are a first-class part of the public surface**: once you have a handle you
//! can call `cursor_at(_mut)` / `remove_slot` / `move_to_front` / `move_to_back` /
//! `pos_of` without knowing the logical position first; the invariants guarantee the
//! handle stays valid **while the element lives** and that inserts/removes do not move
//! other elements. After an element is removed its slot is reused, so **an old handle
//! does not become invalid** — it may point at the new element (classic ABA; for the
//! same-machine comparison and trade-offs see `PERFORMANCE.md` §9). The indices in
//! `head` / `tail` / `free_head` / `free_tail` and `storage` remain private.
//!
//! The three memory layouts share **one implementation**; the only difference is
//! "which fields sit together":
//!
//! | alias | layout | arrangement | per slot (T=8 / T=64) |
//! |---|---|---|---|
//! | [`SplitList`] | [`Split`] | `data` / `prev` / `next` **as three separate `Vec`s** | 24 / 80 B (`u32` index: 16 / 72) |
//! | [`PackedLinksList`] | [`PackedLinks`] | `data` in one `Vec`, `prev`/`next` **paired** into a single `Link` | 24 / 80 B (`u32`: 16 / 72) |
//! | [`NodesList`] | [`Nodes`] | `data` + `prev` + `next` **as a whole node** in one `Node` | 24 / 80 B (`u32`: 16 / 72) |
//!
//! The alias's second parameter is the **index width** ([`Ix`]), defaulting to
//! [`DefaultIx`] = `usize`; enabling the `u32-index` feature switches it to `u32`: a
//! third less per slot and `append` ~32% faster, at the cost of a 2.1G slot limit
//! (see [`Ix::MAX_SLOTS`]). **`PERFORMANCE.md` is measured with this default.**
//!
//! All three are aliases of [`List<T, S>`]: the logic is written once, and the layout
//! differences come from the [`Storage`] strategy.
//!
//! # `no_std`
//!
//! Turning off the default `std` feature gives `no_std` + `alloc`:
//!
//! ```text
//! cargo check --no-default-features     # no_std is only verified with check
//! ```
//!
//! `std` is also a prerequisite for **benches and tests** (criterion, `std::rc` in
//! the tests): under this combination the bench target is skipped via
//! `required-features` and the test module is not compiled. The library itself only
//! uses `core` and `alloc::vec::Vec`, with no other dependencies.
//!
//! # State is transportable (serializable / shared memory)
//!
//! The whole chain's state = the slot arrays plus five numbers, **no pointers**, so it
//! can be written to disk or placed in shared memory directly. The public entry points
//! are [`List::into_raw`] / [`List::from_raw`] (with [`RawList`]'s accessors) and each
//! layout's `as_parts` / `into_parts` / `from_parts`; for per-slot state there is also
//! [`List::iter_slots`] (live) and [`List::free_slots`] (the free chain, LIFO).
//!
//! # Alignment with `std::collections::LinkedList`
//!
//! Both the public API and the semantics align with `LinkedList`:
//!
//! - `new` (`const fn`), `Default`, `len`, `is_empty`, `clear`
//! - `push_front` / `push_back` (returning `()`), `push_front_mut` / `push_back_mut`
//! - `pop_front` / `pop_back`
//! - `front` / `back` / `front_mut` / `back_mut`
//! - `cursor_front` / `cursor_front_mut` / `cursor_back` / `cursor_back_mut`
//! - `iter` / `iter_mut` / `IntoIterator` (`&`, `&mut`, and owned)
//! - `contains`, `retain`, `append`, `split_off`, `remove`
//! - the std-aligned cursor methods: `remove_current_as_list` / `splice_before` /
//!   `splice_after` / `split_before` / `split_after` (**complexity differs from std**, see below)
//! - `Clone`, `Debug`, `PartialEq`, `Eq`, `FromIterator`, `Extend`
//! - cursors: [`Cursor`] / [`CursorMut`], including the "ghost" position, wrap-around
//!   movement, `insert_before` / `insert_after` / `remove_current` / `push_*` / `pop_*`
//!   / `peek_*` / `as_cursor` / `as_list`
//!
//! Semantic differences (all stemming from the single fact that "slots are one
//! contiguous storage"):
//!
//! | operation | std | this library |
//! |---|---|---|
//! | `append` | O(1) pointer splice | O(other's slot count): move the whole run over + add an offset to the indices (the invariant lets each index be written once) |
//! | `split_before` / `split_after` / `split_off` | O(1) pointer split | **O(N) element moves**, and the **moved elements get new slots** (old `Slot` handles become stale) |
//! | `splice_before` / `splice_after` | O(1) | O(number of elements spliced in), inserting one by one |
//! | `remove_current_as_list` | O(1) node move | O(1), but the element **gets a new slot** |
//!
//! `append` in detail:
//!
//! It is not std's O(1) pointer splice; instead it moves the other list's whole run of
//! slots over (adding the offset **as it moves**, so each index is written once),
//! O(other's total slot count); **the other list's free slots come along too**. For
//! "move element by element to the back, preferring to reuse free slots we already
//! have" (billed by live elements, especially worthwhile when the other list is
//! sparse), use [`List::append_elementwise`]. See that method and [`Storage::append`]
//! for the trade-offs between the two paths.
//!
//! # Implementation notes: no sentinel values in the arrays
//!
//! **Every value in the `next` array is a valid slot index** (the "dummy" fields at the
//! ends of the two chains are never read, but are still valid indices); `NIL` is used
//! only as a **scalar** argument (the "empty" value of `head`/`tail`/`free_head`/
//! `free_tail`) and as the cursor's ghost-position tag. This is exactly why
//! [`List::append`] can unconditionally `+= base` the whole run of moved indices — **the
//! free bit rides along**: it sits in the top bit of `prev`, and `2^63 | v` plus `base`
//! is still `2^63 | (v + base)` (as long as `u64` does not overflow).
//!
//! `prev` is a "valid index" only on **live** slots; **a free slot's `prev` is a stale
//! value plus the mark bit** ⇒ hence the contract: **read `prev` only on live slots**
//! ([`Storage::prev`] does not mask; debug builds pin this with `debug_assert`, release
//! pays nothing).
//!
//! To decide whether a slot is live, look only at that mark bit, **never read `data`**:
//! a free slot's `data` is uninitialized, and touching it is UB.
//!
//! This yields two disciplines:
//!
//! - walking a chain **must be by count** (the end dummies are not NIL): [`Iter`] /
//!   [`IterMut`] count with `remaining`, and `retain` / `clear` finish by `len`;
//! - **all four endpoint fields reset to `NIL` when a chain becomes empty**
//!   (`head`/`tail` when `len == 0`, `free_head`/`free_tail` when the free chain empties):
//!   that way "is it empty" is a single field read instead of computing
//!   `storage.slots() - len`, which would run on every free/alloc (measured ~3–6% slower
//!   `churn`); the reset writes two fields only on the transition to empty.
//!
//! # Extensions (not in std)
//!
//! - [`List::at`]: O(min(pos, len-1-pos)) random access; std has no random access at all
//! - [`List::with_capacity`] / [`List::reserve`]: preallocate slot capacity
//!   (nodes live in contiguous memory, so std's list has no such need — ours does)
//! - [`CursorMut::seek`] / [`CursorMut::move_steps`]: move a cursor by position/stride
//! - [`CursorMut::is_head`] / [`CursorMut::is_tail`]: O(1) endpoint checks
//!
//! # Performance
//!
//! Numbers, mechanism explanations, the choice matrix for `append`'s two APIs, and the
//! **optimizations already tried and rejected** are all in **`PERFORMANCE.md`** at the
//! repository root; the benchmarks themselves are in `benches/list.rs`, whose header
//! spells out the reading discipline (noise floor, page-fault counting, alternating A/B).
//!
//! # Intentionally not implemented
//!
//! `PartialOrd` / `Ord` / `Hash`, `extract_if` / `retain_mut`,
//! `CursorMut::splice_before` / `splice_after` / `split_before` /
//! `split_after` / `remove_current_as_list`, and the allocator APIs (`new_in`).

extern crate alloc;
mod cursor;
mod iter;
mod list;
mod slot;
pub mod storage;

#[cfg(all(test, feature = "std"))]
mod tests;

pub use cursor::{Cursor, CursorMut};
pub use iter::{IntoIter, Iter, IterMut};
pub use list::{List, RawList};
pub use slot::Slot;
pub use storage::{DefaultIx, Ix, Nodes, PackedLinks, Split, Storage};

/// `Split` layout: `data` / `prev` / `next` as three independent `Vec`s.
///
/// The second parameter is the **index width** ([`Ix`]: `u8` / `u16` / `u32` / `u64` /
/// `usize`), which directly determines the bytes per slot: with `T = 8`, `usize` is
/// 24 B/slot, `u32` is **16 B**, and `u16` is 12 B. The cost is a slot-count limit
/// (`u32` ⇒ 2.1G, `u16` ⇒ 32k, `u8` ⇒ 128; exceeding it panics).
pub type SplitList<T, I = DefaultIx> = List<T, Split<T, I>>;

/// PackedLinks layout: `data` in one `Vec`, `prev`/`next` interleaved in another `Vec`.
/// The second parameter is the index width (same as [`SplitList`]).
pub type PackedLinksList<T, I = DefaultIx> = List<T, PackedLinks<T, I>>;

/// Nodes layout: `data` / `prev` / `next` in a single `Node`.
/// The second parameter is the index width (same as [`SplitList`]).
pub type NodesList<T, I = DefaultIx> = List<T, Nodes<T, I>>;
