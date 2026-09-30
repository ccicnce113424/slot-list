//! In-memory layout strategies: how `data` / `prev` / `next` land in memory.
//!
//! The three strategies are **algorithmically identical**; only the storage differs:
//!
//! - [`Split`]: three separate `Vec`s (the opposite of array of structs, i.e. structure of arrays)
//! - [`PackedLinks`]: one `Vec` for `data`, `prev`/`next` packed into an interleaved [`Link`]
//! - [`Nodes`]: all three in a single `Node` (array of structs)

use alloc::vec::Vec;
use core::mem::{MaybeUninit, offset_of, size_of};

/// "There is no neighbor on this side."
///
/// **Scalar-only**: it appears as a function argument (`prev` / `next` of
/// `List::insert_link`) and as a cursor's phantom-position tag. It **never appears in the
/// `prev` / `next` arrays** — every value there is a valid slot index (the "dummy" link
/// fields at either end of a list are "valid indices nobody reads", not NIL). This is the
/// premise that lets [`Storage::append`] add an offset to a whole run of indices.
pub(crate) const NIL: usize = usize::MAX;

/// How wide an integer stores slot indices in the link arrays.
///
/// Per-slot cost = `T` + two links, so the link width directly determines memory and
/// bandwidth: switching `usize` to `u32` takes the slot for an 8-byte element (`usize`)
/// from 24 B (8 B payload + 2×8 B links) down to **16 B** (`PERFORMANCE.md` §1.3/§4).
///
/// **Limit**: the free mark bit occupies the top bit (see [`Ix::FREE_BIT`]), and `append`'s
/// wholesale `+= base` avoids the special case by never carrying into that bit, so the
/// usable slot count is `1 << (BITS - 1)`: `u32` => 2.1G, `u16` => 32768, `u8` => 128
/// (default width see [`DefaultIx`]). Exceeding it panics in `grow` / `reserve`, never
/// truncates silently.
///
/// This trait is **sealed** (`u8` .. `u64` / `usize`) and appears in type-parameter position.
pub trait Ix:
    Copy
    + Eq
    + core::ops::Add<Output = Self>
    + core::ops::BitOrAssign
    + core::ops::BitAnd<Output = Self>
    + sealed::Sealed
    + 'static
{
    /// Width (in bits).
    const BITS: u32 = size_of::<Self>() as u32 * 8;

    /// Upper bound on usable slots (= the free mark bit itself, so adding `base` to an
    /// index never carries into the mark bit).
    const MAX_SLOTS: usize = 1 << (Self::BITS - 1);

    /// The "this slot is currently free" mark bit, stored in the **top bit** of `prev`
    /// (`usize` below; narrower indices work the same way with a width of `Ix::BITS - 1`).
    ///
    /// Why it exists: a handle ([`Slot`](crate::Slot)) is a raw identifier pointing at a
    /// slot, and slots are recycled through the free list => "is this slot alive?" must be
    /// O(1), **otherwise we would have to `assume_init_*` the `MaybeUninit` of a free slot
    /// (UB)**.
    ///
    /// Why the top bit of `prev`:
    ///
    /// - **Cheapest to write**: a live slot's `prev` is overwritten by link writes (those
    ///   values never have this bit set), and a free slot only uses `next` in the free
    ///   chain => this bit is written **only on the transition to free**, so the link-write
    ///   path carries no extra work (a direct assignment `prev = FREE_BIT` to save a load
    ///   measured consistently slower on `churn` — see `PERFORMANCE.md` — so the
    ///   read-modify-write stays);
    /// - **`append`'s wholesale `+= base` carries it along by itself**: `2^63 | v` plus
    ///   `base` is still `2^63 | (v + base)`; as long as `u64` does not overflow no special
    ///   case is needed (`v + base < 2^63` and the slot count is far below that => always
    ///   true in practice, with a `debug_assert` as backstop);
    /// - **`prev` is read only on live slots** (a contract, not advice): a free slot's
    ///   `prev` *is* this mark bit (`FREE_BIT | base` after `append`), so masking would add
    ///   an `and` at every read site and lengthen the address dependency chain.
    ///   [`Storage::prev`] therefore **returns the raw value** and pins "the slot must be
    ///   live" with a `debug_assert` in debug builds (zero cost in release).
    ///
    /// Marking free is written **only on the transition to free**, so its form sits
    /// directly on `churn`'s hot path. Direct assignment and a single-byte write both
    /// measured slower than the current `prev |= FREE_BIT` (see `PERFORMANCE.md`); the
    /// gaps come from whole-function codegen rearrangement, not one instruction.
    ///
    /// Used as a **boolean** (`raw & FREE_BIT != 0`) it has no such problem: LLVM lowers it
    /// to `mov %rdi,%rax; shr $63,%rax` (two instructions, no immediate).
    ///
    /// Cost: one read-modify-write per slot that becomes free (once on every reclamation
    /// path: `pop`/`remove`/`clear`, etc.).
    const FREE_BIT: Self;

    /// Zero (used to test the mark bit without relying on `PartialEq<{integer}>`).
    const ZERO: Self;

    /// Widen the index to `usize`.
    fn to_usize(self) -> usize;

    /// Narrow a `usize` slot index to this width.
    ///
    /// Debug builds assert that `value` fits, i.e. stays below `1 << (BITS - 1)`
    /// (see [`Ix::MAX_SLOTS`]).
    fn from_usize(value: usize) -> Self;
}

/// Cold path for hitting the limit.
///
/// **Deliberately** `#[cold] #[inline(never)]` and a message with no format arguments:
/// `grow` is part of the hot path (`alloc_slot` must inline into it). A `{}` argument
/// would drag the formatting machinery into `grow`, after which the inliner gives up on
/// inlining `alloc_slot` — measured ~47% slower `churn` (a `call` appears in the loop).
#[cold]
#[inline(never)]
fn ix_overflow() -> ! {
    panic!("slot count exceeds the index width limit (see Ix::MAX_SLOTS)")
}

macro_rules! impl_ix {
    ($($t:ty),* $(,)?) => {$(
        impl sealed::Sealed for $t {}

        impl Ix for $t {
            const FREE_BIT: Self = 1 << (<$t>::BITS - 1);
            const ZERO: Self = 0;

            #[inline]
            fn to_usize(self) -> usize {
                self as usize
            }

            #[inline]
            fn from_usize(value: usize) -> Self {
                debug_assert!(
                    value < Self::MAX_SLOTS,
                    "slot index exceeds the selected index width: {value} >= {}",
                    Self::MAX_SLOTS
                );

                value as $t
            }
        }
    )*};
}

impl_ix!(u8, u16, u32, u64, usize);

/// The raw-address layout that [`Iter`](crate::Iter) / [`IterMut`](crate::IterMut) need.
///
/// The three `Storage`s place the same three logical fields in completely different spots,
/// and the iterators must walk the chain through raw addresses (so `next()` can hand out
/// `&'a mut T` without fighting its own state). This unifies all three under a single
/// "**base + stride + offset**" formula (all indices are slot indices):
///
/// ```text
/// data address of element i = data.cast::<u8>() + i * data_stride + data_offset
/// prev value of element i   = *(prev.cast::<u8>() + i * prev_stride)
/// next value of element i   = *(next.cast::<u8>() + i * next_stride)
/// ```
///
/// Fields (all bases are **byte pointers** `u8`, so each access needs only **one** cast:
/// `u8` -> target type. Marking a base as `*mut MaybeUninit<T>` would instead require a
/// `cast::<u8>()` for byte arithmetic and a cast back — two):
/// - `data`: the **base** of element 0's `data` slot (a slot may be uninitialized; call
///   `assume_init_*` before reading);
/// - `data_stride`: bytes between the `data` of adjacent elements (= the size of one "element");
/// - `data_offset`: bytes to add to the `data` base to reach element 0's `data` field
///   (non-zero only for layouts that tuck the field inside a node, and computed with
///   `offset_of!`, so it does not rely on the assumption that `data` sits at the start);
/// - `prev` / `next`: the base of element 0's link fields (links are **always `usize`** in
///   all three layouts, so the type is fixed and only the stride varies);
/// - `prev_stride` / `next_stride`: bytes between the link fields of adjacent elements —
///   in PackedLinks the two links are packed into a `Link`, and in Nodes they share a
///   `Node<T>` with `data`, so the stride here is the "element" size rather than 8.
///
/// How each layout maps onto these fields (`T = usize`, index `u32`; parenthesized values
/// are for `T = [u64; 8]`, measured):
///
/// | layout | `data` base / `data_stride` / `data_offset` | `prev`·`next` base / stride |
/// |---|---|---|
/// | `Split<T, I>` | data array start / `size_of::<T>()` 8 (64) / 0 | each array start / `size_of::<I>()` 4 |
/// | `PackedLinks<T, I>` | data array start / 8 (64) / 0 | `links + offset_of!(Link<I>, …)` 0·4 / 8 |
/// | `Nodes<T, I>` | node array start / `size_of::<Node<T, I>>()` 16 (72) / `offset_of!(Node<T, I>, data)` 0 | `nodes + offset_of!(Node<T, I>, …)` 8·12 / 16 (72) |
///
/// ```text
/// Split      data  [d0][d1][d2]…      prev [p0][p1]…      next [n0][n1]…
///                 ↑ stride 8               ↑ stride 4           ↑ stride 4
/// PackedLinks   data  [d0][d1]…          links [(p0,n0)][(p1,n1)]…
///                                           ↑ prev=links+0, next=links+4, stride 8
/// Nodes      nodes [(d0,p0,n0)][(d1,p1,n1)]…
///                  ↑ data=nodes+0, prev=nodes+8, next=nodes+12, stride 16
/// ```
///
/// "Two index domains": the `data` domain follows `Layout`'s stride (width set by the
/// layout), while the **link domain is always `Ix` width** (8 bytes by default, see
/// [`DefaultIx`]) — [`IterMut`](crate::IterMut) uses `ix_width` to decide how to read.
///
/// Computing addresses this way rests on three invariants: (1) the pointers come from the
/// container's own `Vec`, and `Layout` is only used during the exclusive period when an
/// iterator holds `&'a mut List` (no realloc / relocation in between); (2) indices stay
/// within allocated slots; (3) every `prev`/`next` in the arrays is a **valid index** (the
/// two ends are "self-referential dummy" links, see the crate docs), so they can be read
/// unconditionally and `+= base` applied unconditionally — which is exactly the premise for
/// [`Storage::append`]'s wholesale relocation.
#[doc(hidden)]
pub struct Layout {
    pub(crate) data: *mut u8,
    pub(crate) data_stride: usize,
    pub(crate) data_offset: usize,
    pub(crate) prev: *const u8,
    pub(crate) prev_stride: usize,
    pub(crate) next: *const u8,
    pub(crate) next_stride: usize,
    /// Byte width of a link element (1/2/4/8): the iterator uses it to read and step
    /// through links at raw addresses.
    pub(crate) ix_width: usize,
}

#[doc(hidden)]
pub mod sealed {
    /// Seals [`super::Storage`] so external types cannot implement it.
    pub trait Sealed {}
}

/// Storage strategy.
///
/// This trait is **sealed**: only the three layouts of this crate implement it. It is an
/// implementation detail of [`List`](crate::List) and appears in type-parameter position.
pub trait Storage<T>: sealed::Sealed {
    /// Total number of allocated slots (live + free).
    fn slots(&self) -> usize;

    /// Whether there are no slots at all.
    fn is_empty(&self) -> bool {
        self.slots() == 0
    }

    /// Reserve capacity for at least `additional` more slots (`len + additional`).
    fn reserve(&mut self, additional: usize);

    /// Return the backing `Vec`'s **excess capacity** to the allocator. The **slot count
    /// does not change** (free slots are part of the free chain and are what handles point
    /// at, so they cannot be dropped) => invariants, `Slot` handles and chain structure are
    /// all untouched.
    fn shrink_to_fit(&mut self) {
        // Default: no-op (a custom layout may not support it)
    }

    /// Append a new slot and return its index.
    ///
    /// Both links are initialized to self-loops first: the caller rewrites the ones it needs
    /// right after, but **the two "dummy" link fields at the ends may keep this value
    /// forever**, so it must be a valid index (see [`append`](Storage::append): every index
    /// in the run is offset unconditionally).
    fn grow(&mut self) -> usize;

    /// Append all of `other`'s slots after `self` and add an **offset to every index moved
    /// over** (offset = `self.slots()` before the move, taken by the implementation itself).
    /// Afterwards `other` is empty (its capacity stays with it).
    ///
    /// The key is "copy and write the **already-updated** index into one's own `Vec` at the
    /// same time": each index is written once, which measured faster than a bulk memcpy
    /// followed by an in-place read-modify-write pass (~0.7 ms/24 MB), and even faster than
    /// a plain three-array memcpy, because it saves a 16 MB read + 16 MB write.
    ///
    /// The measured cost is dominated by allocator behavior, not the copy: with mimalloc (the
    /// crate's benchmark default) a 1M ⊕ 1M append of 24 MB of payload runs in ~3.8-4.3 ms
    /// with zero page faults, whereas the system malloc pays 4095-8187 page faults and
    /// 7.9-16 ms. A control experiment that `mmap`s 24 MB and writes one byte per page with
    /// no copying at all takes 8.2 ms / 5860 pages under both allocators, so this path is
    /// billed **per page**: faulting, not copying, is the dominant cost (glibc switches to
    /// mmap/munmap above 128 KB; mimalloc reuses segments and purges lazily), which also
    /// explains how one and the same append can drift from 2 ms to 16 ms — compare page-fault
    /// counts alongside timings. Only touching the target memory beforehand avoids it — fill
    /// the elements into existing free slots, i.e. [`crate::List::append_elementwise`]
    /// (measured ~1.8 ms). `madvise(MADV_HUGEPAGE)` would in theory erase the cost (5860 pages
    /// -> 239 in the best case), but this machine runs THP in `madvise` mode with
    /// `nr_hugepages=0`, so the re-measurement is unstable and it is not something to count on.
    ///
    /// Three negative results (all measured; do not retry):
    ///
    /// - `map(|i| i + base)` matches `extend_from_slice` => the index rewrite has no room for
    ///   improvement;
    /// - a `u32` index cuts this path by ~32% (3.67 → 2.48 ms for a 1M ⊕ 1M `SplitList` append
    ///   in the benchmarks' `index_width` group) — fewer bytes per slot is real here;
    /// - **`reserve` before append has no measurable benefit** — std's `Vec::append`/`extend`
    ///   already reserves then copies, and spelling it out (3 runs per allocator) gives
    ///   identical time and fault counts; conversely `reserve_exact` shaves off the
    ///   amortization headroom and makes the immediate next `push` pay a full relocation again
    ///   (measured 3.9->7.5 ms, 15.5->34.0 ms, i.e. **doubled**).
    fn append(&mut self, other: &mut Self);

    /// The slot's element storage, as `MaybeUninit`: a slot may be uninitialized,
    /// and reading it before `assume_init_*` is UB.
    fn data(&self, slot: usize) -> &MaybeUninit<T>;
    /// Mutable access to the slot's element storage (same caveat as [`Storage::data`]).
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T>;
    /// Whether the slot is currently free. **Reads only the top bit of `prev`**, never
    /// `data` (a free slot's `data` is uninitialized, and touching it is UB).
    fn is_free(&self, slot: usize) -> bool;

    /// Mark the slot free (called when pushing it back onto the free chain). Repeated
    /// marking is idempotent.
    ///
    /// The implementation sets the top bit ([`Ix::FREE_BIT`]) of `prev` in place and does
    /// nothing beyond that read-modify-write — `append`'s wholesale `+= base` turns it into
    /// `FREE_BIT | base`, and the mark bit remains.
    fn mark_free(&mut self, slot: usize);

    /// Predecessor index of the slot.
    ///
    /// **Contract: only call this on a slot with `!is_free(slot)`** (a free slot's `prev` is
    /// the free mark bit itself and is meaningless). Hence there is **no masking** here:
    /// masking would add an `and` at every read site and lengthen the address dependency
    /// chain; out-of-range misuse is caught by a `debug_assert` in debug builds.
    fn prev(&self, slot: usize) -> usize;
    /// Successor index of the slot.
    fn next(&self, slot: usize) -> usize;
    /// Set the predecessor index of the slot.
    fn set_prev(&mut self, slot: usize, value: usize);
    /// Set the successor index of the slot.
    fn set_next(&mut self, slot: usize, value: usize);

    #[doc(hidden)]
    fn layout(&mut self) -> Layout;
}

// ============================================================
// Default index width
// ============================================================

/// Default index width: **`usize`**.
///
/// The `u32-index` feature switches it to `u32` (one third fewer bytes per slot, ~32% faster
/// `append`, limit see [`Ix::MAX_SLOTS`]) — a switch for "run the full test suite /
/// benchmarks under a narrow index", **off by default**.
#[cfg(feature = "u32-index")]
pub type DefaultIx = u32;
/// [`DefaultIx`] without the `u32-index` feature (the default configuration).
#[cfg(not(feature = "u32-index"))]
pub type DefaultIx = usize;

// ============================================================
// Split: data, prev and next each in their own array
// ============================================================

/// **Three completely separate streams**: one `Vec` each for `data` / `prev` / `next`.
///
/// Walking the chain touches only the index arrays (8 B per link with the default `usize`
/// index, 4 B with `u32`) — the most bandwidth-efficient option; the cost is three base
/// addresses per slot, so an element and its links never share a cache line.
///
/// The index width is adjustable (`prev`/`next` narrow together): `Split<T, u16>` /
/// `Split<T, u32>` / `Split<T, usize>`, default see [`DefaultIx`].
pub struct Split<T, I = DefaultIx> {
    data: Vec<MaybeUninit<T>>,
    prev: Vec<I>,
    next: Vec<I>,
}

impl<T, I: Ix> Split<T, I> {
    /// Creates an empty layout with no allocated slots.
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
        }
    }
}

impl<T, I: Ix> Split<T, I> {
    /// Raw parts (read-only): `(data, prev, next)`, all three the same length = slot count.
    ///
    /// **This is the entry point for movable state**: the whole state of a chain = these
    /// arrays + those five numbers ([`List::into_raw`](crate::List::into_raw));
    /// serialization, persistence and shared memory all take it from here.
    pub fn as_parts(&self) -> (&[MaybeUninit<T>], &[I], &[I]) {
        (&self.data, &self.prev, &self.next)
    }

    /// Raw parts (taking ownership), to be paired with [`Self::from_parts`].
    pub fn into_parts(self) -> (Vec<MaybeUninit<T>>, Vec<I>, Vec<I>) {
        (self.data, self.prev, self.next)
    }

    /// Rebuild from raw parts.
    ///
    /// # Safety
    ///
    /// The caller guarantees: the three arrays have equal length; every `prev`/`next` is a
    /// **valid slot index** (invariant: `NIL` is never written into the arrays, the two end
    /// dummies are self-loops); **the top bit of a free slot's `prev` is the free mark**
    /// ([`Ix::FREE_BIT`]), otherwise `is_free` decides wrongly and `assume_init_drop` steps
    /// on uninitialized data; a live slot's `data` must be initialized. Violating this is
    /// UB, not a panic.
    pub unsafe fn from_parts(data: Vec<MaybeUninit<T>>, prev: Vec<I>, next: Vec<I>) -> Self {
        debug_assert_eq!(
            data.len(),
            prev.len(),
            "from_parts: data/prev lengths differ"
        );
        debug_assert_eq!(
            data.len(),
            next.len(),
            "from_parts: data/next lengths differ"
        );

        Self { data, prev, next }
    }
}

impl<T, I: Ix> Default for Split<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for Split<T, I> {}

impl<T, I: Ix> Storage<T> for Split<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.data.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.data.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.data.reserve(additional);
        self.prev.reserve(additional);
        self.next.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.prev.shrink_to_fit();
        self.next.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.data.push(MaybeUninit::uninit());
        // dummy: a self-loop (a valid index, see the trait docs)
        self.prev.push(I::from_usize(slot));
        self.next.push(I::from_usize(slot));

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        // Confirm up front that the whole thing is in range (so the per-element base
        // addition below never carries into the mark bit)
        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

        // data has no indices to fix => move it wholesale (empties other.data)
        self.data.append(&mut other.data);
        // The two index arrays are copied and incrementally offset: written once.
        // The addition happens **in the narrow integer domain**, not through `from_usize`:
        // a free slot's `prev` still carries the free mark (`FREE_BIT | v`), and adding the
        // base yields `FREE_BIT | (v + base)` — which is exactly why `prev` needs no
        // separate mark-bit fixup (see the `FREE_BIT` docs).
        self.prev.extend(other.prev.iter().map(|&ix| ix + base));
        self.next.extend(other.next.iter().map(|&ix| ix + base));

        other.prev.clear();
        other.next.clear();
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(slot) }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(slot) }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        // As a **boolean** the immediate is not a concern: LLVM lowers "and the top bit"
        // to a single shift (see the FREE_BIT docs).
        unsafe { (*self.prev.get_unchecked(slot) & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        debug_assert!(!self.is_free(slot), "prev may only be read on a live slot");
        unsafe { self.prev.get_unchecked(slot).to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.next.get_unchecked(slot).to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { *self.next.get_unchecked_mut(slot) = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: self.prev.as_ptr() as *const u8,
            prev_stride: size_of::<I>(),
            next: self.next.as_ptr() as *const u8,
            next_stride: size_of::<I>(),
            ix_width: size_of::<I>(),
        }
    }
}

// ============================================================
// PackedLinks: one Vec for data, prev/next packed into a Link
// ============================================================

/// The two links of one slot in the `PackedLinks` layout.
///
/// Public because the **raw-parts API** (`PackedLinks::as_parts` / `from_parts`) hands it
/// out — serializing a chain yourself or restoring it from shared memory requires reading
/// and writing this type.
#[derive(Clone, Copy, Debug)]
pub struct Link<I> {
    /// Predecessor slot index (a free slot's top bit is the free mark, see [`Ix::FREE_BIT`]).
    pub prev: I,
    /// Successor slot index.
    pub next: I,
}

/// **Only the indices are interleaved in pairs**: one `Vec` for `data`, `prev`/`next`
/// packed into a single `Link` array.
///
/// Walking the chain keeps both links on one cache line (a single miss fetches predecessor
/// and successor), while `data` stays separate => checking the free mark touches only the
/// indices, never the elements.
///
/// The index width is adjustable (the two fields share one `Link`, so they narrow together).
/// Default see [`DefaultIx`].
pub struct PackedLinks<T, I = DefaultIx> {
    data: Vec<MaybeUninit<T>>,
    links: Vec<Link<I>>,
}

impl<T, I: Ix> PackedLinks<T, I> {
    /// Creates an empty layout with no allocated slots.
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl<T, I: Ix> PackedLinks<T, I> {
    /// Raw parts (read-only): `(data, links)`, both arrays the same length = slot count.
    pub fn as_parts(&self) -> (&[MaybeUninit<T>], &[Link<I>]) {
        (&self.data, &self.links)
    }

    /// Raw parts (taking ownership), to be paired with [`Self::from_parts`].
    pub fn into_parts(self) -> (Vec<MaybeUninit<T>>, Vec<Link<I>>) {
        (self.data, self.links)
    }

    /// Rebuild from raw parts.
    ///
    /// # Safety
    ///
    /// Same requirements as [`Split::from_parts`]: the two arrays have equal length; every
    /// `prev`/`next` is a valid slot index; the top bit of a free slot's `prev` is the free
    /// mark ([`Ix::FREE_BIT`]); a live slot's `data` is initialized.
    pub unsafe fn from_parts(data: Vec<MaybeUninit<T>>, links: Vec<Link<I>>) -> Self {
        debug_assert_eq!(
            data.len(),
            links.len(),
            "from_parts: data/links lengths differ"
        );

        Self { data, links }
    }
}

impl<T, I: Ix> Default for PackedLinks<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for PackedLinks<T, I> {}

impl<T, I: Ix> Storage<T> for PackedLinks<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.links.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.data.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.data.reserve(additional);
        self.links.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.links.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.data.push(MaybeUninit::uninit());
        self.links.push(Link {
            prev: I::from_usize(slot),
            next: I::from_usize(slot),
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

        self.data.append(&mut other.data);
        // Both links are packed into one array, so they too are written only once (the
        // addition happens in the narrow integer domain; a free slot's `prev` carries the
        // mark bit, see the FREE_BIT docs)
        self.links.extend(other.links.iter().map(|link| Link {
            prev: link.prev + base,
            next: link.next + base,
        }));

        other.links.clear();
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(slot) }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(slot) }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        unsafe { (self.links.get_unchecked(slot).prev & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        debug_assert!(!self.is_free(slot), "prev may only be read on a live slot");
        unsafe { self.links.get_unchecked(slot).prev.to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.links.get_unchecked(slot).next.to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).next = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        let links = self.links.as_ptr() as *const u8;

        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: unsafe { links.add(offset_of!(Link<I>, prev)) },
            prev_stride: size_of::<Link<I>>(),
            next: unsafe { links.add(offset_of!(Link<I>, next)) },
            next_stride: size_of::<Link<I>>(),
            ix_width: size_of::<I>(),
        }
    }
}

// ============================================================
// Nodes: all fields in a single Node
// ============================================================

/// One slot in the `Nodes` layout: element and both links share a cache line.
///
/// Public for the same reason as [`Link`] (raw-parts API).
pub struct Node<T, I> {
    /// The element; uninitialized on a free slot (**do not** `assume_init`).
    pub data: MaybeUninit<T>,
    /// Predecessor slot index (a free slot's top bit is the free mark, see [`Ix::FREE_BIT`]).
    pub prev: I,
    /// Successor slot index.
    pub next: I,
}

impl<T, I> Node<T, I> {
    /// Assemble one slot.
    pub fn new(data: MaybeUninit<T>, prev: I, next: I) -> Self {
        Self { data, prev, next }
    }
}

/// **Data and indices interleaved per node**: one `Node` per slot (`data` + `prev` + `next`)
/// in a single `Vec`.
///
/// `next` shares a cache line with `data` (getting the index brings the element along) =>
/// cheapest for briefly visiting elements; the cost is that **even checking the free mark
/// pays the bandwidth of a whole node** (see the density table of
/// [`List::clear`](crate::List::clear): its scan threshold is the highest).
///
/// Default index width see [`DefaultIx`].
pub struct Nodes<T, I = DefaultIx> {
    nodes: Vec<Node<T, I>>,
}

impl<T, I: Ix> Nodes<T, I> {
    /// Creates an empty layout with no allocated slots.
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<T, I: Ix> Nodes<T, I> {
    /// Raw parts (read-only): `nodes` (each node carries its element and both links).
    pub fn as_parts(&self) -> &[Node<T, I>] {
        &self.nodes
    }

    /// Raw parts (taking ownership), to be paired with [`Self::from_parts`].
    pub fn into_parts(self) -> Vec<Node<T, I>> {
        self.nodes
    }

    /// Rebuild from raw parts.
    ///
    /// # Safety
    ///
    /// Same requirements as [`Split::from_parts`]: every node's `prev`/`next` is a valid slot
    /// index; the top bit of a free slot's `prev` is the free mark ([`Ix::FREE_BIT`]); a live
    /// slot's `data` is initialized.
    pub unsafe fn from_parts(nodes: Vec<Node<T, I>>) -> Self {
        Self { nodes }
    }
}

impl<T, I: Ix> Default for Nodes<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for Nodes<T, I> {}

impl<T, I: Ix> Storage<T> for Nodes<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.nodes.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.nodes.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.nodes.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.nodes.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.nodes.push(Node {
            data: MaybeUninit::uninit(),
            prev: I::from_usize(slot),
            next: I::from_usize(slot),
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

        // data and both links live in the same array, so "one pass" means data has to be
        // moved element by element too (no memcpy); written in the least-effort way and
        // left to the compiler's best effort.
        self.nodes.extend(other.nodes.drain(..).map(|node| Node {
            data: node.data,
            prev: node.prev + base,
            next: node.next + base,
        }));
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { &self.nodes.get_unchecked(slot).data }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { &mut self.nodes.get_unchecked_mut(slot).data }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        unsafe { (self.nodes.get_unchecked(slot).prev & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).prev.to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).next.to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).next = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        let nodes = self.nodes.as_ptr() as *const u8;

        Layout {
            // The base is the node array itself; `data_offset` moves the address to the `data` field
            data: self.nodes.as_mut_ptr() as *mut u8,
            data_stride: size_of::<Node<T, I>>(),
            data_offset: offset_of!(Node<T, I>, data),
            prev: unsafe { nodes.add(offset_of!(Node<T, I>, prev)) },
            prev_stride: size_of::<Node<T, I>>(),
            next: unsafe { nodes.add(offset_of!(Node<T, I>, next)) },
            next_stride: size_of::<Node<T, I>>(),
            ix_width: size_of::<I>(),
        }
    }
}
