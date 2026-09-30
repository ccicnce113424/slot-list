# `slot-list` performance summary

`List<T, S>` is a doubly linked list where **the index is the pointer**: nodes live in a
preallocated array of contiguous slots, and removed slots are threaded onto a free list for reuse.
The three memory layouts (`SplitList` / `PackedLinksList` / `NodesList`) are three `Storage`
parameters of the same generic type. The baselines are std's `LinkedList`, `VecDeque` and `Vec`, plus
`fast-list` 0.1.8 in §7.

Every number is one **median** from the configuration in §1, measured for 0.1.0. §3 holds the tables,
§4 is the short guide to picking a layout and tuning it.

---

## 0. Overview

All figures are **1M elements of `usize`** (8 B payload, 24 B/slot, default `usize` index) unless
noted.

| Question | Answer |
|---|---|
| End operations `push`+`pop` (no preallocation) | 2.1-4.9× slower than `VecDeque`, roughly on par with `LinkedList` (5.1-10.0 vs 5.3-5.7 ms); this group is dominated by **page faults and bytes copied** (§3.1) |
| Steady-state end operations `churn` (no allocation) | **2.0× faster than `LinkedList`** (2.77 vs 5.61 ms), 1.7× slower than `VecDeque` (1.65 ms) (§3.4) |
| Iteration | On par with `LinkedList` (1.11 vs 0.99 ms; ~1.1 ns/element = the dependent-load floor); **1.57× faster at a 64 B payload** (361 vs 568 µs, 250k × `Blob64`) (§3.2) |
| Middle insert/remove | On par with `LinkedList` (538 vs 473 µs), ~150× faster than `VecDeque` (81.6 ms; it memmoves 500k elements every time) (§3.3) |
| Memory/slot | **24 B for 8 B of payload (33% payload)**; 16 B with the `u32` index; 72-80 B at a 64 B payload (§1.3) |
| Key differences from std `LinkedList` | ① no per-node allocation (why `churn` / `blob_end_ops` win); ② `append` is not an O(1) pointer splice (§3.6) |
| Unique value (beyond performance) | **stable handle `Slot` (O(1) entry/remove/move, ~39,000× faster than going by position)**, zero allocation on the hot path, relocatable state, swappable layout (§6) |
| Most effective knobs | preallocating capacity (`with_capacity`) and reusing free slots (`append_elementwise`), then the allocator / huge pages (§4) |

Throughput is not the selling point: "chunking" is the unrolled-linked-list family ≈
`LinkedList<Vec<T>>`, which is faster on every one of these items but has no element-level cursor and
no stable handle (§6).

---

## 1. Method, environment and element types

### 1.1 Environment these numbers come from

| | |
|---|---|
| CPU | AMD Ryzen 7 7840H (Zen 4), 8 cores / 16 threads, 16 MiB L3, `performance` governor |
| RAM, OS | 14 GiB; NixOS, Linux 7.2.8, x86-64 |
| Rust | rustc / cargo 1.101.0-nightly (2026-09-26) |
| Global allocator | **mimalloc 0.1.52** (the `mimalloc` feature). Plain `cargo bench` uses the system `malloc` instead; see below |
| Features, index width | `cargo bench --features mimalloc,linked-list-cursors`; default `DefaultIx = usize` (the `u32` comparison is §3.8) |
| Build profile | cargo `bench`: `opt-level = 3`, LTO off, debug assertions off, no `target-cpu` override |
| Harness | criterion 0.8, `sample_size(10)`, warm-up 500 ms, measurement 1 s, **median**, single-threaded single process |

Every number is a same-machine A/B on the machine above. The allocator is part of the configuration:
`churn`, `iteration`, `cursor_update`, `middle_*` and `slot_entry` do not move with it, while
`end_ops`, `append*` and `blob_*` allocate inside the timed region and can move by several-fold in
either direction under the system allocator, so do not mix the two. The CI artifacts
(`.github/workflows/benches.yml`, published for every dispatch) are a different machine class again,
and the guest CPU model varies between dispatches, so they support structural conclusions and A/B
inside one run only.

### 1.2 Element types used below

| name | Rust type | payload bytes | used by |
|---|---|---|---|
| `usize` | `usize` (one machine word) | 8 | every 1M-element group |
| `Blob64` | `[u64; 8]` (opaque; `key()` reads word 0) | 64 | the 250k "large payload" groups |
| `Drop64` | `[u64; 8]` with a `Drop` that reads `self.0[0]` | 64 | the "glue" rows of `clear_drop` |

### 1.3 What "per slot" means

A slot holds **one element plus its two links**. All three layouts add up to the same total; they
differ only in where the fields sit (§2). The index width `Ix` sets the link size.

| element (payload) | index width | payload | links (`prev`+`next`) | slot size | payload share |
|---|---|---|---|---|---|
| `usize` | `usize` (8 B), default | 8 B | 16 B | **24 B** | 33% |
| `usize` | `u32` (4 B) | 8 B | 8 B | **16 B** | 50% |
| `usize` | `u16` (2 B) | 8 B | 4 B | **12 B** | 67% |
| `[u64; 8]` (`Blob64` / `Drop64`) | `usize` (8 B), default | 64 B | 16 B | **80 B** | 80% |
| `[u64; 8]` | `u32` (4 B) | 64 B | 8 B | **72 B** | 89% |

For contrast, the baselines store no links: `Vec<usize>` and `VecDeque<usize>` are **8 B per
element** (plus growth slack), and `LinkedList<usize>` is **8 B payload + two 8 B pointers + the
allocator's per-node header (~16 B) ⇒ ≥ 32 B per node**, allocated separately. That accounting is
behind every throughput difference in §3.

### 1.4 Reading discipline

- **Every number is one median from one run.** Compare two versions only inside one run, alternating
  (A/B/A/B) and with an external baseline measured in the same run. The noise floor is ±5% for the
  small-working-set groups (`iteration`, `churn`, `cursor_update`) and ±10-20% for the
  large-allocation ones (`append*`, `blob_*`).
- **criterion keys results by group and bench name**: after a rename or with a different feature set
  the old leaves stay on disk and `base`/`change` compare against them, so delete `target/criterion`
  (or use `--save-baseline`) before comparing runs.
- **Scales**: `N` = 1M elements, `LARGE_N` = 250k. The element type follows the group (`usize` at N,
  `Blob64` at `LARGE_N`).

---

## 2. The three layouts: memory and addresses

`Layout` describes addresses as base + stride + offset (the index is a slot number):

```text
data address of element i = data base + i * data_stride + data_offset
prev value of element i   = *(prev base + i * prev_stride)
next value of element i   = *(next base + i * next_stride)
```

| Layout | Memory/slot (`usize` index) | `data_stride` | `data_offset` | `prev`·`next` stride | Structure |
|---|---|---|---|---|---|
| `Split<T, I>` | `T` + 16 B | `size_of::<T>()` | 0 | 8 | three separate `Vec`s |
| `PackedLinks<T, I>` | `T` + 16 B | `size_of::<T>()` | 0 | 16 (`Link<I>`) | `data` interleaved with `Link{prev,next}` |
| `Nodes<T, I>` | `size_of::<Node<T, I>>()` | same as the slot | 0 | same as the slot | a single `Vec<Node<T, I>>` |

Concretely, for the element types used below (`size_of` / `offset_of!`): `usize` (8 B) is 24 B in all
three layouts, `[u64; 8]` (64 B) is 80 B in all three; with the `u32-index` feature those become 16 B
and 72 B, and the link strides 4 and 8. `Nodes` stores `data` first, and **the crate never relies on
field order**: every offset comes from `offset_of!` (`src/storage.rs`).

---

## 3. Full results

Unless a table says otherwise: element = `usize` (8 B payload, 24 B/slot), `N` = 1M, default `usize`
index, environment §1.1.

### 3.1 End operations `end_ops` (1M pushes + 1M pops)

Element = `usize`, N = 1M, **no preallocation**, so the slot array grows while the pushes run.

| Shape (1M pushes + 1M pops, `usize` elements, no preallocation) | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `push_back` + `pop_front` | 5.90 ms | 5.43 ms | 5.10 ms | 2.38 ms | 5.28 ms |
| `push_front` + `pop_back` | 9.99 ms | 5.83 ms | 5.76 ms | 2.03 ms | 5.67 ms |

This group measures "grow-and-copy plus the first touch of every page". Per element we allocate 24 B
(8 B payload + 16 B links) against `VecDeque`'s 8 B, so three times the pages and the bytes copied,
which is where `VecDeque`'s 2.0-2.4 ms advantage mostly comes from. The `SplitList` cell in the
second row (9.99 ms) sits at the edge of the ±10-20% floor; the rest is 5.1-5.9 ms, i.e. roughly
`LinkedList`'s level. **Do not read a layout ordering out of this group.**

### 3.2 Iteration `iteration` / `blob_iter`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList | Vec |
|---|---|---|---|---|---|---|
| `iteration`: 1M × `usize` (8 B payload) | **1.11 ms** | 1.38 ms | 1.57 ms | 440.1 µs | 986.5 µs | 75.9 µs |
| `blob_iter`: 250k × `Blob64` (64 B payload) | **360.9 µs** | 423.2 µs | 560.2 µs | 190.8 µs | 567.6 µs | 181.0 µs |

- **~1.1 ns/element is the chain walk**, i.e. the dependent-load latency of "read `next` → go to that
  slot" (~4 cycles/element). The iterator adds nothing measurable on top of it. That floor is
  inherent to a linked structure and is why iteration is the one item where the compact layouts
  (`Split`, `PackedLinks`) win: pulling in fewer bytes per step means fewer cache misses.
- **With a large payload we beat `LinkedList`** (1.57×): each of its 64 B elements is a separate node
  allocation plus a pointer chase, while our contiguous 80 B slots let the hardware prefetcher run.
  `Nodes` gains the least here, because the whole 80 B node is pulled in per step.
- The numbers depend on **chain locality**: this chain is built with `push_back`, so slots are
  ascending and the prefetcher guesses right; after `random_remove_insert` it is fragmented and
  `iter()` pays about one miss per step.

### 3.3 Locating and middle insert/remove `middle_access` / `middle_insert_remove`

Element = `usize`, N = 1M.

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `middle_access`: 100 locates at N/2 | 53.37 ms | 54.93 ms | 64.85 ms | 56.8 ns | 50.79 ms |
| `middle_insert_remove`: 1000 insert+remove at N/2 | 537.7 µs | 555.8 µs | 646.3 µs | 81.63 ms | 473.0 µs |

`at(pos)` walks `O(min(pos, len-1-pos))` steps, so each "call" in `middle_access` walks 500k steps,
about 1.1 ns per step, the same cost as §3.2. `VecDeque`'s 56 ns is an O(1) index, not the same
operation; that column only marks the gap.

`middle_insert_remove` inserts, steps and removes 1000 times in the middle, and **all three layouts
are on `LinkedList`'s scale** (within 1.4×). The bottleneck is slot allocation/free-list traffic,
not the chain, so the layout differences nearly vanish. `VecDeque` needs 81.6 ms because every
middle insert is a memmove.

### 3.4 Steady state and perturbation `churn` / `random_remove_insert` / `cursor_update`

Element = `usize`, N = 1M. `churn` and `cursor_update` operate on a list built up front, so no
allocation happens inside the timing region.

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `churn`: 1M × pop_front+push_back | 2.77 ms | 2.74 ms | 2.95 ms | 1.65 ms | 5.61 ms |
| `random_remove_insert`: 100 random-position remove+insert | 28.33 ms | 31.61 ms | 34.30 ms | 4.39 ms | 27.27 ms |
| `cursor_update`: 1M in-place read/writes at the chain midpoint | 1.20 ms | 1.20 ms | 1.35 ms | 662.4 µs | 2.26 ms |

- `churn` **allocates nothing at all**, so it measures **pure code**: each `pop_front`+`push_back`
  touches ~4-8 link fields (two neighbours plus the free list) where `VecDeque` touches two indices,
  which is the whole 1.7× gap; `LinkedList` `malloc`s/`free`s every node, so we are 2.0× faster.
  This is **the most direct payoff of "contiguous slots + free list"**.
- `random_remove_insert` fragments the chain and randomizes positions ⇒ all three layouts land at
  28-34 ms (`Split` best). `VecDeque` needs only 4.4 ms because a middle insert is a memmove for it,
  while we must walk the chain.
- `cursor_update` is the other side of the same trade: `Nodes` is slowest (1.35 ms; the interleaved
  24 B node costs an extra hop to reach `data`), then `LinkedList` (2.26 ms, one pointer chase per
  step), then `VecDeque`.

### 3.5 Large payload `blob_end_ops` (element = `Blob64`, 64 B payload, 80 B/slot, 250k)

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `blob_end_ops`: 250k × `Blob64` push + pop | 3.93 ms | 3.50 ms | 3.75 ms | 2.86 ms | 4.16 ms |

The link overhead amortizes with payload: 64 B of an 80 B slot (80%) against 8 B of 24 B (33%).
`VecDeque` leads by only 1.37× (against 2.1-4.9× at 8 B, §3.1) and **we beat `LinkedList`**, whose
every 64 B element is a separate allocation.

### 3.6 `append`: two APIs (where this crate differs from std's semantics)

The two APIs **bill different units**; pick by shape:

- `append` (mirrors `LinkedList::append`): moves **all of the other list's slots** as one block,
  adding the offset to indices as it goes. It bills **the other list's slot count + your own storage
  size**, independent of the other side's live element count.
- `append_elementwise` (this crate's extension): moves element by element, preferring to fill your
  **existing free slots**. It bills **the live element count**.

`LinkedList::append` is an O(1) pointer splice (69.6 ns for 1M ⊕ 1M, independent of payload and of
slot size). We gave that up for iteration locality (contiguous slots), **a deliberate design
choice**.

| Shape (both sides built by push/collect, so the target must grow) | SplitList | PackedLinksList | NodesList | VecDeque | Vec | LinkedList |
|---|---|---|---|---|---|---|
| `append`: 1M ⊕ 1M `usize` (24 B/slot) | 4.08 ms | 3.62 ms | 3.42 ms | 1.17 ms | 946.6 µs | 69.6 ns |
| `append_blob`: 250k ⊕ 250k `Blob64` (80 B/slot) | 2.64 ms | 2.79 ms | 2.71 ms | 2.00 ms | 1.85 ms | 44.6 ns |
| same, `::elementwise` (target pre-built with 250k live + 250k free slots) | 1.77 ms | 1.85 ms | **1.65 ms** | - | - | - |

- **The gap to `Vec::append` is the byte count, not the index rewrite**: a block move copies 24 B per
  element against `Vec`'s 8 B, and because both sides grew, every byte moves twice. Rewriting the
  indices on the way is free; the lever is the **link width** (§3.8) or not growing at all.
- **Close it by preallocating** the target (`with_capacity` / `reserve`), which avoids the second move
  and the fresh pages: this is the single biggest knob (§4).
- **Large payload narrows the gap**: at 64 B slots the link overhead is 20% instead of 67%, so we are
  1.43× off `Vec` and `append_elementwise` wins outright when the target already has free slots
  (1.65-1.85 ms against 2.64-2.79 ms).

### 3.7 Handle entry vs position entry (`slot_entry` group)

Element = `usize`, N = 1M. **The same work, differing only in how the element is found**: 100 ×
"remove at a random position, insert back in place" (`cursor_at(handle)` + `remove_current` +
`insert_before` vs `at(pos)` + the same two steps):

| Entry | SplitList | PackedLinksList | NodesList |
|---|---|---|---|
| by handle `cursor_at` (`O(1)`) | 719 ns | 611 ns | **547 ns** |
| by position `at(pos)` (walks the chain `O(min(pos, len-1-pos))`) | 28.33 ms | 31.60 ms | 34.26 ms |

Totals for 100 operations, i.e. **~6-7 ns vs ~283-343 µs per operation, a gap of about 39,000×**;
the only difference is whether the entry point has to walk the chain to find the element.

### 3.8 Index width: `u32` vs `usize` (the `u32-index` feature)

Same run, 1M `usize` elements, `index_width` group (element `usize`, so the slot is 24 B with the
default index and 16 B with `u32`):

| Bench | `u32` | `usize` (default) | Ratio |
|---|---|---|---|
| `append` 1M ⊕ 1M | **2.48 ms** | 3.67 ms | `usize`/`u32` = 1.48× |
| `churn` | 2.73 ms | 2.74 ms | 1.00× |
| `iteration` | 1.10 ms | 1.16 ms | 1.05× |
| `middle_access` | 53.24 ms | 53.31 ms | 1.00× |

The whole gain is the bytes moved: `append` moves 33% less data per element, everything else is
within ±1%. The price is the slot ceiling: the top bit of the index holds the free bit, so `u32`
caps the list at `2^31 - 1` slots (`Ix::MAX_SLOTS`). `u16` (12 B/slot) is available through the same
parameter for small lists.

---

## 4. Selection guide

**Layout**

- Iteration-heavy (many reads, few writes) → `SplitList` / `PackedLinksList` (`Nodes` iterates ~55%
  slower at a 64 B payload).
- Frequent reads/writes at middle positions → `PackedLinksList` (`prev`/`next` share one `Link`,
  hence one cache line).
- Simplest allocation and memory / elements ≤ 48 B → `NodesList` (one `Vec`, one realloc).
- Elements ≥ 64 B and memory matters → `SplitList` / `PackedLinksList` (8 B less per slot).

**The two `append` APIs**

- Target **has** free slots, or the source is sparse → `append_elementwise` (billed per live element;
  three orders of magnitude faster on a sparse source).
- Target must grow and the source is dense → `append` (a block move runs at bandwidth over bytes,
  independent of the element count).

**The two biggest knobs**

1. `with_capacity` / `reserve`: preallocate the target so it never grows ⇒ `append` **−50~60%**.
2. Reusing already-touched slots (`append_elementwise`): no fresh pages ⇒ orders of magnitude on a
   cold allocator.

**Environment (outside the crate, but the biggest influence on `append*`)**

```sh
cargo bench                                          # system malloc (glibc): watch page faults, not wall clock
cargo bench --features mimalloc                      # mimalloc: the configuration behind most numbers here
GLIBC_TUNABLES=glibc.malloc.hugetlb=1 cargo bench    # glibc >= 2.35, route large blocks through huge pages
MIMALLOC_PURGE_DELAY=-1 cargo bench                  # mimalloc: don't give pages back to the kernel
```

---

## 5. Reproduction

```sh
cargo bench --features mimalloc,linked-list-cursors   # the configuration of this document
cargo bench                                          # system malloc (glibc)
cargo bench -- 'FastList'                            # only the fast-list comparison group (§7)
cargo bench -- 'clear_drop'                          # clear/Drop comparison
cargo bench -- 'iteration|churn'                     # filter (regex)
cargo test                                           # correctness (37 tests, including per-slot invariant checks)
cargo test --release -- --ignored --nocapture probe_clear_vs_scan   # clear's density curve
cargo miri test                                      # strict provenance (the iterator goes through raw addresses)
```

`benches/list.rs` compiles on stable as well: the comparison rows against std `LinkedList` cursors
(the `LinkedList` columns of `middle_insert_remove` / `random_remove_insert` / `cursor_update`) need
`#![feature(linked_list_cursors)]`, gated behind the **`linked-list-cursors` feature** (**not in
`default`**) ⇒ `cargo bench` works directly on stable, and on nightly
`cargo bench --features linked-list-cursors` adds those three rows. A feature rather than a
`build.rs` channel probe, because this is a library: `build.rs` would run for every downstream user
compiling this crate, while a feature is explicit, costs downstream nothing, and matches `mimalloc`.

---

## 6. Beyond throughput

The shape of this crate is chosen for the four properties below, not for winning `iteration` or
`append` (chunking, i.e. the unrolled-linked-list family, is faster on those; see the table at the
end of this section).

1. **Stable slot identity, exposed as a handle ([`Slot`])**: insert/remove never moves other
   elements, and a slot stays put for the element's whole lifetime.
   - **Getting a handle**: `Cursor::slot` / `CursorMut::slot` / `List::front_slot` /
     `List::back_slot` / `List::iter_slots` (all `O(1)`); **using one**: `List::cursor_at` /
     `cursor_at_mut` / `remove_slot` / `move_to_front` / `move_to_back` (all `O(1)`, no logical
     position needed).
   - **Measured gain** (§3.7): ~6-7 ns/op by handle against ~283-343 µs/op by position, i.e.
     **about 39,000×**.
   - **"Is it live?" is decidable in `O(1)`**: the free bit lives in the top bit of `prev`, and the
     test never touches uninitialized `data`. The link-write path and `append`'s bulk `+= base` both
     carry it along (see the `FREE_BIT` documentation in `src/storage.rs`).
   - **Trade-off: no generation check.** A handle answers only "is this slot live right now"; once
     the free list reuses a slot, an old handle points at the **new element** (classic ABA, pinned by
     a test). If stale handles must be detectable, use `slotmap` / `fast-list`, at the cost of
     running 1.5-1.9× slower on same-machine benchmarks (§7.1).
2. **Zero allocation on the hot path**: once capacity is in place, `pop_front`/`push_back` never
   `malloc` (`churn_does_not_allocate`: 1M churns of a `usize` list = **0** allocations across all
   three layouts; `LinkedList` in the same test does 1,000,000). After `clear`, slots go back onto
   the free list instead of to the kernel, so memory and latency stay predictable.
3. **Relocatable, pointer-free state**: the whole chain is per-slot `(T, prev, next)` plus
   `head`/`tail`/`free_head`/`free_tail`/`len` (`state_is_relocatable`: moved verbatim into a new
   container, iteration and invariants identical) ⇒ it can be serialized, shared or `mmap`ed with no
   pointer fixup. Entry points: [`List::into_raw`] / [`List::from_raw`] plus each layout's
   `as_parts` / `into_parts` / `from_parts` and the per-slot `iter_slots()` / `free_slots()`.
4. **Layout is a parameter**: `Split`/`PackedLinks`/`Nodes` share one implementation, and `Layout`
   (base + stride + offset) unifies them into the raw addresses the iterator wants. Changing layout
   is a one-type-parameter edit (trade-offs in §2 and §3).

⇒ **This crate is not "the fastest linked list" but "the one you can use as the storage engine of a
`LinkedHashMap` / LRU while keeping an element-level list API, stable handles and deterministic
memory".** For chunk-level throughput use `LinkedList<Vec<T>>` (or put chunks in as elements:
`List<Vec<T>, Split<Vec<T>>>`); same machine and session, 1M `usize`:

| Metric | `SplitList` (24 B/slot) | `LinkedList<Vec<usize>>` K=64 (~9 B/element) | Ratio |
|---|---|---|---|
| iteration | 1.11 ns/element | **0.112** | 9.9× |
| `at(N/2)` (walk to the middle) | ~1.1 ns/element | **0.013** | ~85× |
| `churn` (`pop_front`+`push_back`) | 2.77 ns/op | **1.70** | 1.6× |
| `end_ops` (push 1M + pop 1M, no preallocation) | 5.90 ns/op | **0.93** | 6.3× |
| memory/slot | 24 B | **~9 B** | 2.7× |
| element-level cursor / stable slot identity | ✓ | ✗ | - |

---

## 7. Comparison with `fast-list` 0.1.8

The closest opponent in this niche: slotmap index + generation (`LinkedListIndex`). Every fact below
comes from both sides' source; the numbers are medians from one same-machine `cargo bench` (the
`FastList` series in `benches/list.rs`), same run and configuration as §3, **element = `usize`
(8 B payload), N = 1M, default `usize` index, environment §1.1**.

### 7.1 Performance

| Group (scale) | this crate's `SplitList` (24 B/slot) | `FastList` (`LinkedListItem<usize>` = 32 B + slotmap metadata) | Ratio | `LinkedList` (≥ 32 B/node) | `VecDeque` (8 B/element) |
|---|---|---|---|---|---|
| `end_ops/push_back_pop_front` (1M pushes + 1M pops) | **5.90 ms** | 11.28 ms | 1.9× | 5.28 ms | 2.38 ms |
| `iteration` (1M elements) | **1.11 ms** | 1.77 ms | 1.6× | 0.99 ms | 0.44 ms |
| `middle_access` (100 × N/2) | **53.37 ms** | 79.95 ms | 1.5× | 50.79 ms | 56.8 ns |
| `middle_insert_remove` (1000 insert/removes at N/2) | **538 µs** | 814 µs | 1.5× | 473 µs | 81.6 ms |
| `churn` (1M × pop+push) | **2.77 ms** | 5.10 ms | 1.8× | 5.61 ms | 1.65 ms |
| `random_remove_insert` (100 random positions) | **28.33 ms** | 42.39 ms | 1.5× | 27.27 ms | 4.39 ms |
| `cursor_update` (1M midpoint read/writes) | **1.20 ms** | 1.65 ms | 1.4× | 2.26 ms | 0.66 ms |
| `slot_entry::by_handle` (100 handle entries) | **719 ns** | 42.11 ms | **58,000×** | - | - |
| `slot_entry::by_pos` (100 position entries) | **28.33 ms** | 42.29 ms | 1.5× | - | - |

Reading note: `end_ops` measures the allocator (§1.4, ±20%); the other groups ±5%.

**Why the `by_handle` cell differs by ~58,000×** (generation semantics, not implementation quality):

- The benchmark body is "remove at a random position, insert back in place", and criterion runs that
  loop repeatedly.
- `fast-list`'s `LinkedListIndex` carries a generation: after a `remove` plus a re-insert **the old
  handle is dead**. From the second round on, the only way out is, once `contains_key` reports death,
  **to find the element by position** (O(N)), which is what the benchmark does, making `by_handle` ≈
  `by_pos`.
- This crate's `Slot` is a raw index: after `remove` + `insert_before` the slot is reused LIFO, **so
  the old handle is still valid** and still points at that cell (ABA semantics), hence O(1). The
  price: it will not tell you that the element has been swapped out.

⇒ The difference is not "who is faster" but **handle semantics after mutation**: they guarantee
"invalidation is detectable", we guarantee "identity never moves". To use `fast-list` well on such
workloads you must maintain an external "element → current handle" map.

### 7.2 Features

| Dimension | this crate | `fast-list` 0.1.8 |
|---|---|---|
| Handle type | `Slot` (raw `usize` index, 8 B, no generation) | `LinkedListIndex` (slotmap key: `u32` index + `u32` generation, 8 B) |
| Stale handle / ABA | ✗ **undetectable**: after a slot is reused the old handle points at the new element (pinned by a test) | ✓ `contains_key()` validates immediately |
| Handle → element | ✓ `cursor_at(_mut)` / `remove_slot`, O(1) | ✓ `get` / `get_mut` / `remove`, O(1) |
| Position → handle | ✓ `slot_at(pos)`, O(N) | ✓ `nth(pos)`, O(N) |
| **Element-level cursor** | ✓✓ `Cursor`/`CursorMut`: survives insert/remove, ghost position, `move_prev/next/steps`, `peek` neighbours | ✗ only `cursor_next/prev` (helpers for taking neighbours while traversing) |
| "Where was I" after a removal | ✓ the cursor stays in place, `insert_before` puts it back | ✗ use `LinkedListItem::next_index`/`prev_index` yourself as the anchor |
| `move_to_front` / `move_to_back` | ✓ built in, O(1) | ✗ needs `insert_*` + `remove`, and **produces a new index ⇒ the old handle dies** |
| Bulk move `append` | ✓ O(1) slot renumbering plus a range-wide index offset | ✗ only `extend` (push one by one) |
| Attached data | ✗ none built in (but `Slot` is an ordinary index, usable as a key into any map) | ✓ `new_data::<V>()` / `new_data_sparse::<V>()` (slotmap's `SecondaryMap`) |
| Whole-chain state import/export | ✓ `into_raw` / `from_raw` + `as_parts` / `from_parts` + `iter_slots` / `free_slots` (round-trip test) | ✗ only copying element by element via `iter()` |
| Unordered traversal | ✓ `iter_slots() -> (Slot, &T)` | ✓ `iter_unordered() -> &LinkedListItem<T>` |
| Ordered traversal / `retain` / `split_off` | ✓ | ✓ |
| Find by **value** `contains` | ✓ | ✗ (only the by-handle `contains_key`) |
| Layout choice | ✓ three (Split / PackedLinks / Nodes) | ✗ one |
| Memory per slot (`usize` element) | **24 B** = 8 B payload + 8 B `prev` + 8 B `next` (`u32`: 16 B; §1.3, §2) | `LinkedListItem<usize>` = **32 B** (8 B value + `u32` index + `u32` generation + `u32` next + `u32` prev + padding), **plus** slotmap's per-slot metadata |
| Zero allocation on the hot path | ✓ `churn_does_not_allocate` | ✓ pinned in the same test |
| Visible `unsafe` | the core is unchecked reads/writes plus a raw-address iterator, backed by miri and the full test suite | **0 occurrences** ⇒ cheaper to audit |
| Dependencies / `no_std` | none; turning off the `std` feature gives `no_std` + `alloc` | `slotmap` (+ optional `unstable` for `Walker`); `core` only, does not declare `no_std` |
| Derived impls | `Clone` / `Debug` / `PartialEq` / `Eq` / `Default` / `FromIterator` / `Extend` / `IntoIterator` | only `Debug` |

**Conclusion**: two trade-offs of the same data structure, not replacements for each other.

- Need a **generation check** (stale handles must be detectable), want slotmap's `SecondaryMap` to
  carry payload alongside, or want one less `unsafe` ⇒ use `fast-list`; the price is larger slots
  (32 B + metadata vs 24 B), handles dying on every insert/remove, and no cursor / `move_to_*` /
  `append`.
- Need **cursor semantics** (staying in place after a removal, ghost position, relative movement),
  **three layouts**, **bulk moves**, **smaller slots**, **handles that survive mutation** ⇒ use this
  crate; the price is that stale handles are **undetectable** (bring your own generation counter, or
  maintain an external map).
