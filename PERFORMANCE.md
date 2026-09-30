# `slot-list` performance summary

`List<T, S>` is a doubly linked list where **the index is the pointer**: nodes live in a
preallocated array of contiguous slots, and removed slots are threaded onto a free list for reuse.
The three memory layouts (`SplitList` / `PackedLinksList` / `NodesList`) are three `Storage`
parameters of the same generic type. The baselines are std's `LinkedList`, `VecDeque` and `Vec`.

---

## 0. Overview

| Question | Answer (full suite, 1M elements) |
|---|---|
| End operations `push`+`pop` (no preallocation) | 2.4–3.0× slower than `VecDeque`, on par with `LinkedList` (6.1–7.5 vs 5.8–6.0 ms) — this group is dominated by **page faults and bytes copied** (§3.1) |
| Steady-state end operations `churn` (no allocation) | **2.2× faster than `LinkedList`** (2.75 vs 6.15 ms), 1.6× slower than `VecDeque` (§3.4) |
| Iteration | On par with `LinkedList` (1.21 vs 1.05 ms; ~1.1 ns/element = the dependent-load floor); **1.74× faster with a 64 B payload** (§3.2) |
| Middle insert/remove | On par with `LinkedList`, 150× faster than `VecDeque` (which memmoves 500k elements every time) (§3.3) |
| Memory/slot | **24 B** (T=8, default `usize` index; the `u32` index is 16 B); all three layouts agree at 80 B for T=64 (§2) |
| Key differences from std `LinkedList` | ① no per-node allocation (why `churn` / `blob_end_ops` win); ② `append` is not an O(1) pointer splice (§3.6) |
| Confirmed at the ceiling | **within this crate's shape**: walking the chain during iteration and `append`'s copying (already at bandwidth) — three routes were implemented/modelled and rejected (§4). Going faster means changing the shape: chunking ≈ `LinkedList<Vec<T>>`, faster on every item but with no element-level cursor and no stable handle (end of §4) |
| Unique value (beyond performance) | **stable handle `Slot` (O(1) entry/remove/move, measured ~40,000× faster than going by position)**, zero allocation on the hot path (tested), relocatable state (tested), swappable layout — see §8 |
| Most effective knobs | reusing free slots (`append_elementwise`) and preallocating capacity (`with_capacity`), then the allocator / huge pages (§6) |

---

## 1. Method and reading discipline

> **Index width**: most numbers in this document were measured with the **`u32` index** (16 B/slot
> at T=8, which is what the `append` −37% figure belongs to); the **default is now `usize`**
> (24 B/slot, so that −37% no longer applies). Run either configuration with `cargo bench` (default
> `usize`) or `cargo bench --features u32-index` (`u32`); the benchmark prints the active width at
> startup.

- Single-threaded `cargo bench` (criterion, `sample_size(10)`, `measurement_time(1s)`, medians).
- **Noise floor: ±5%** (less for small working sets such as `iteration` / `churn` / `cursor_update`);
  **±10–20%** for the large-allocation groups `append*` / `blob_*` — when the machine drifts, the
  same `churn` code moves from 2.80 to 3.01 ms while the `VecDeque` baseline does not budge, so
  **always read the baseline alongside**.
- **`end_ops` / `churn` / `append*` measure the allocator and the kernel, not the code**: the same
  `append` code swings from 2 ms to 16 ms (page faults 0 → 8187). Compare **page faults inside the
  timing region**.
- Regression calls: run the two versions **alternating** (A/B/A/B), with an external baseline
  (`Vec`/`VecDeque`/`LinkedList`) as reference; only a difference beyond the noise floor counts.
- **Bandwidth-sensitive vs latency-sensitive**: purely bandwidth-bound items (`clear`'s scan arm,
  `append*`, sequential scans) can inflate **3–4×** on the same machine when something else is
  running (a full-table scan measured 0.13 ms ↔ 0.65 ms), whereas latency-bound items (`churn`,
  cursors, chain-walking, which sits at ~1.08 ms) move only ~25% ⇒ **take the reference within the
  same run**; never divide two columns measured at different times.
- Scales: `N` = 1M elements (`usize`, 8 B); `LARGE_N` = 250k (`Blob64`, 64 B).

---

## 2. The three layouts: memory and addresses (measured and checked)

`Layout` describes addresses as base + stride + offset (the index is a slot number):

```text
data address of element i = data base + i * data_stride + data_offset
prev value of element i   = *(prev base + i * prev_stride)
next value of element i   = *(next base + i * next_stride)
```

| Layout | Memory/slot (T=8 / T=64, `u32` index) | `data_stride` | `data_offset` | `prev`·`next` stride | Structure |
|---|---|---|---|---|---|
| `Split<T, I>` | **16 / 72** B | 8 / 64 | 0 | 4 | three separate `Vec`s |
| `PackedLinks<T, I>` | **16 / 72** B | 8 / 64 | 0 | 8 (`Link<I>`) | `data` interleaved with `Link{prev,next}` |
| `Nodes<T, I>` | **16 / 72** B | 16 / 72 | 0 | 16 / 72 | a single `Vec<Node<T, I>>` |

(The table is for the **`u32` index**; with the **default `usize`** every layout adds 8 B/slot —
`Split`/`PackedLinks` 24 B, `Nodes` 24 B, and 80 B for T=64, where a `Node` crossing a cache line
leaves no padding to save. `data_offset` is 0 for all three layouts at both widths.)

On the field order of `Node<T>` (`repr(Rust)`):

- The compiler **may reorder fields**. This repository measures it keeping the declaration order
  `[data][prev][next]` (all three fields are aligned ≤ 8 and no padding is available to save, so
  the order is stable), hence `data_offset` is 0.
- **The crate does not rely on this**: offsets always come from `offset_of!`. Moving `data` into
  the middle makes `data_offset` 8, and all 20 tests still pass unchanged (the iterator goes
  through `Layout`).
- Both orders measured: the cache-line sets are **identical step by step** (`next` is always at
  `T+8`, `prev`/`data` merely swap) and the performance difference is inside the noise. The only
  risk is an **over-aligned** `T` (align ≥ 16) with `#[repr(C)]`, where centering `data` grows the
  node by 50% (32→48, 64→96); under `repr(Rust)` the compiler avoids this itself.

---

## 3. Full results

> Default index width `usize` (24 B/slot); the `u32-index` feature switches to `u32` (16 B/slot).
> The tables below were measured at 1M elements; `iteration` and `churn` are the most sensitive to
> the width and the rest stay within ±5% noise. The three-layer breakdown in §3.2 and the byte
> accounting in §3.6 are `usize`-era magnitudes.

### 3.1 End operations `end_ops` (1M pushes + 1M pops)

The same `push_back`/`pop_front` and `push_front`/`pop_back` flows across five implementations:

| Shape (1M pushes + 1M pops, no preallocation) | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `push_back` + `pop_front` | 6.08 ms | 6.44 ms | 6.13 ms | 2.53 ms | 5.79 ms |
| `push_front` + `pop_back` | 7.49 ms | 6.24 ms | 5.64 ms | 2.10 ms | 5.99 ms |

This group does **not** preallocate, so it measures "grow-and-copy plus the first touch of every
page": 16 B/slot (against `VecDeque`'s 8 B) ⇒ three times the pages and the bytes copied, and that
is where `VecDeque`'s 2.1–2.5 ms advantage mostly comes from, not from faster code. Layout
differences within the group (at most ±23%) sit inside the group's ±10–20% noise floor — do not
over-read them.

### 3.2 Iteration `iteration` / `blob_iter`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList | Vec |
|---|---|---|---|---|---|---|
| `iteration` (1M × 8 B, full traversal) | 1.21 ms | **1.10 ms** | **1.32 ms** | 310.4 µs | 1.05 ms | 80.7 µs |
| `blob_iter` (250k × 64 B) | 378.8 µs | 473.0 µs | 562.7 µs | 240.6 µs | 660.4 µs | 240.9 µs |

Splitting iteration into three layers (internal probe, min of 9 rounds, ns/element):

| Layout | public `iter()` | walking the chain by hand | sequential slot scan (no chain) |
|---|---|---|---|
| SplitList | 1.08 | 1.14 | **0.062** |
| PackedLinksList | 1.31 | 1.37 | **0.063** |
| NodesList | 1.29 | 1.34 | **0.478** |
| `Vec` sequential scan (reference) | — | — | 0.062 |

- **The iterator itself wastes nothing**: `iter()` is only ~5% slower than walking the chain by
  hand (a two-level check plus a counter).
- **Sequential slot scan: Split/PackedLinks match `Vec`**; **`Nodes` degrades 7.7×** (interleaved
  nodes use only 8 of every 24 B). But no public API scans all slots in slot order ⇒ that is only a
  ceiling, not the cost of any call.
- **The expensive part is walking the chain**: ~1.1 ns/element is the **dependent-load latency** of
  "read `next` → go to that slot" (~4 cycles/element), not bandwidth. This is the inherent floor of
  a linked structure, and three improvement routes were tried (§4).
- **With a large payload it beats `LinkedList`**: `blob_iter` 378.8 µs vs 660.4 µs (1.74× faster) —
  `LinkedList` allocates each node separately, so every 64 B element costs a pointer chase and
  possibly a page fault, while our contiguous 80 B slots let the hardware prefetcher run.
- The numbers depend on **chain locality** (this chain is built with `push_back`, so it is
  0,1,2,… ascending and the prefetcher guesses right); after `random_remove_insert` the chain is
  fragmented and `iter()` drops to roughly one miss per step.

### 3.3 Locating and middle insert/remove `middle_access` / `middle_insert_remove`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `middle_access` (100 locates at N/2) | 54.02 ms | 55.61 ms | 66.96 ms | 56.8 ns | 59.77 ms |
| `middle_insert_remove` (1000 inserts + removes) | 546.9 µs | 569.0 µs | 670.4 µs | 82.46 ms | 495.8 µs |

`at(pos)` walks `O(min(pos, len-1-pos))` steps, so each "call" in `middle_access` actually walks
500k steps over a 1M-element chain ≈ 0.5 ms — about 1 ns per step, exactly matching the chain-walk
cost in §3.2. `VecDeque`'s 56 ns is an O(1) index; the two are not operations of the same order, and
the column is there only to mark the gap.

`middle_insert_remove` = insert + step + remove 1000 times in the middle of the chain, and **all
three layouts are on `VecDeque`'s scale**: the bottleneck here is not the chain but **allocating and
freeing slots** (random accesses popping/pushing the free list), so the layout difference — where
each per-slot array sits — is nearly invisible.

### 3.4 Steady state and perturbation `churn` / `random_remove_insert` / `cursor_update`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `churn` (1M × pop_front+push_back) | 2.75 ms | 3.00 ms | 2.72 ms | 1.68 ms | 6.15 ms |
| `random_remove_insert` (100 random-position removes + inserts) | 28.55 ms | 32.44 ms | 37.52 ms | 4.60 ms | 32.18 ms |
| `cursor_update` (1M in-place read/writes at the chain midpoint) | 1.19 ms | 1.26 ms | 1.34 ms | 672.2 µs | 2.45 ms |

- `churn` **allocates nothing at all** (the list is built up front and slots cycle), so it measures
  **pure code**: each `pop_front`+`push_back` touches ~4–8 link fields (two neighbours plus the
  free list) while `VecDeque` touches only two indices ⇒ the whole 1.63× gap is here; and since
  `LinkedList` `malloc`s/`free`s every node, we are 2.3× faster — **the most direct payoff of
  "contiguous slots + free list"**.
- `random_remove_insert` fragments the chain and randomizes positions ⇒ all three layouts land at
  28–38 ms (`Split` best) while `VecDeque` needs only 4.6 ms — middle insert/remove is a memmove for
  it, whereas we must walk the chain (`at(pos)` is O(N)).
- `cursor_update` reads and writes a point in the middle of the chain in place: `Nodes` is slowest
  (1.34 ms, interleaved nodes force an extra hop to reach `data` each time), `LinkedList` 2.45 ms
  (one pointer chase per step).

### 3.5 Large payload `blob_end_ops` (`Blob64`, 64 B/element, 250k)

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `blob_end_ops` (250k × 64 B push + pop) | 4.40 ms | 3.99 ms | 4.22 ms | 3.24 ms | 4.59 ms |

With a large payload the link overhead amortizes: `VecDeque` leads by only 1.36× (vs 2.4–3.0× at
8 B), and **we beat `LinkedList`** (4.2 vs 4.6 ms), whose every 64 B node is a separate allocation.

80 B/slot (64 B payload + 16 B links) ⇒ the payload is 80% of it. This is the archetypal "our link
overhead amortizes with a large payload" case: at 8 B elements, 24 B/slot is 3× the payload; at
64 B it is only 25% more.

### 3.6 `append`: two APIs (where this crate differs from std's semantics)

The two APIs **bill different units**; pick by shape:

- `append` (mirrors `LinkedList::append`): moves **all of the other list's slots** as one block
  (adding the offset to indices as it goes). It bills **the other list's slot count + your own
  storage size**, independent of the other side's live element count.
- `append_elementwise` (this crate's extension): moves element by element, preferring to fill your
  **existing free slots**. It bills **the live element count**.

`LinkedList::append` is an O(1) pointer splice (measured 144 ns, independent of payload); we gave
that up for iteration locality (contiguous slots) — **a deliberate design choice**.

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | Vec | LinkedList |
|---|---|---|---|---|---|---|
| `append` (1M ⊕ 1M, both sides must grow) | 4.53 ms | 4.01 ms | 3.88 ms | 1.37 ms | 1.15 ms | 172.1 ns |
| `append_blob` (250k ⊕ 250k, 64 B elements) | 3.24 ms | 3.24 ms | 2.93 ms | 2.19 ms | 2.06 ms | 178.9 ns |
| same, `::elementwise` (target has 250k free slots up front) | 1.76 ms | 1.90 ms | 1.75 ms | — | — | — |

**Is a block `append` 2.6× slower than `Vec::append`? Byte accounting (mimalloc, 1M⊕1M):**

| Shape | Time | Bytes actually moved | Effective bandwidth |
|---|---|---|---|
| `SplitList::append` (3×usize, must grow) | 3.84 ms | ~96 MB | 25.0 GB/s |
| `Vec<usize>::append` (must grow) | 1.45 ms | ~32 MB | 22.1 GB/s |
| bare Vec×(usize, u32, u32) (must grow) | 2.82 ms | ~64 MB | 22.7 GB/s |
| bare Vec×3 usize (target capacity up front, no growth) | 1.93 ms | ~48 MB | 24.8 GB/s |
| `Vec<usize>` (target capacity up front, no growth) | 0.60 ms | ~16 MB | 26.6 GB/s |
| hot target 8 MB `extend_from_slice` | 0.58 ms | ~16 MB | 27.4 GB/s |
| hot target 8 MB `extend(map(\|i\| i + base))` | 0.52 ms | ~16 MB | 30.9 GB/s |

- **Index rewriting is free**: `extend(map(|i| i + base))` runs at the same speed as
  `extend_from_slice` (even faster, as it skips the memcpy library call) ⇒ adding the offset is not
  a source of cost.
- The whole gap is **the number of bytes moved**: 16 B/slot (data+prev+next, `u32` index) against
  `Vec`'s 8 B, and both sides move every byte twice because of growth (`realloc` moves the old data,
  then the append copies it) = 4× the payload. Every measurement lands in the same bandwidth band
  (21–31 GB/s), and our per-byte figure is even slightly better.
- Only two ways to close it (both measured): drop the index to `u32` (**supplied by the
  `u32-index` feature**) ⇒ 16 B/slot, **−37%**; preallocate the target so it never grows ⇒ **−57%**
  (the latter requires the allocator to keep pages hot).

**The cost of a block move is almost entirely "does the target need fresh pages"** (same shape, only
the environment changes):

| Environment | SplitList | PackedLinksList | NodesList | Page faults in the timing region |
|---|---|---|---|---|
| mimalloc (`--features mimalloc`) | 3.7 ~ 4.3 ms | 3.6 ~ 4.0 ms | 3.4 ~ 3.7 ms | 0 |
| system malloc (no `mimalloc` feature) | 8.2 ms | 17.0 ms | 11.3 ms | 4095 / 8095 / 12003 |
| system malloc + `GLIBC_TUNABLES=glibc.malloc.hugetlb=1` | 2.8 ms | 4.2 ms | 2.4 ms | 8 / 17 / 24 |
| reference: `mmap` 24 MB, write 1 byte per page, **no copying** | 8.2 ~ 9.4 ms | — | — | 5860 |

⇒ this path bills **per page** (~1.2 µs for the first touch of a page on this machine). For "how
fast is the algorithm itself", read the mimalloc row (0 page faults); when comparing versions
**always look at the page-fault count as well**.

**Large payload (`Blob64`, 250k):**

| Shape | Time | Moved | Bandwidth |
|---|---|---|---|
| SplitList | 2.87 ~ 3.20 ms | ~80 MB | 25.0 GB/s |
| PackedLinksList | 2.86 ~ 2.94 ms | ~80 MB | 28.0 GB/s |
| NodesList | 2.80 ~ 2.92 ms | ~80 MB | 28.6 GB/s |
| VecDeque | 2.22 ms | ~64 MB | 28.8 GB/s |
| `Vec` (baseline) | 1.98 ~ 2.40 ms | ~64 MB | 26.7 GB/s |
| `LinkedList` (O(1) pointer splice) | 144 ns | 0 | — |
| reference: three bare arrays with capacity up front (no growth) | 1.17 ms | ~40 MB | 34.2 GB/s |

The gap shrinks with a large payload (2.6× slower than `Vec` at 8 B, only ~1.2–1.35× at 64 B: the
extra 16 B/slot is just 20% there); the layout difference only becomes visible now, and is still
small (`Nodes` ≈ `PackedLinks` ≲ `Split`, 5–12%).

**Both APIs on the same target shape** (target has 250k free slots up front; mimalloc, 80 B/slot):

| Shape | `append_elementwise` | `append` |
|---|---|---|
| target has 250k free slots + source 250k dense | **1.78 ms** | 3.83 ms |
| target has 250k free slots + source sparse (250k slots / 100 live) | **0.001 ms** | 3.59 ms |
| target 0 free slots + source 250k dense | 3.07 ms | **2.85 ms** |
| target 0 free slots + source sparse | **0.001 ms** | 2.40 ms |

The per-element cost is approximately linear in the payload: **≈ 2 ns + 0.08 ns × payload bytes per
element** (every element is moved twice, out of the source and into the target, so this is not
streaming bandwidth): measured 2.1 ns at 8 B, 6.9 ns at 64 B, 22.4 ns at 256 B. A block move, by
contrast, just runs at bandwidth over bytes (21–28 GB/s) regardless of the element count. So in the
"target cannot hold it" cell the two paths converge as the payload grows.

**Both APIs × input shapes (target fixed at 1M live elements, N = 1M, min of 3 rounds, allocator
warmed up)**:

| Target free slots \ Source | dense 1M | sparse 1M slots / 100 live | dense 100 |
|---|---|---|---|
| 0 | element-wise 4.9 / **block 3.5** → use `append` | element-wise **0.001** / block 3.4 → **`append_elementwise` is mandatory** | ~0 / ~0 |
| 1 000 | element-wise 4.7 / **block 3.5** → `append` | **element-wise 0.001** / block 4.1 | ~0 / ~0 |
| 1M | **element-wise 2.1** / block 4.9 | **element-wise 0.001** / block 4.4 | ~0 / ~0 |


### 3.7 Handle entry vs position entry (`slot_entry` group)

**The same work, differing only in how the element is found**: 100 × "remove at a random position,
insert back in place" (`cursor_at(handle)` + `remove_current` + `insert_before` vs `at(pos)` + the
same two steps):

| Entry | SplitList | PackedLinksList | NodesList |
|---|---|---|---|
| by handle `cursor_at` (`O(1)`) | **0.8 µs** | **0.6 µs** | **0.6 µs** |
| by position `at(pos)` (walks the chain `O(min(pos, len-1-pos))`) | 31.7 ms | 34.9 ms | 39.4 ms |

(Units are totals for 100 operations; per operation: **~8 ns vs ~317–394 µs**, a gap of about
**40,000×**.) The by-handle column is the steady state where the 100 victim slots sit in cache (the
common case in an LRU); even with cold victim slots it is only a few extra misses (hundreds of ns),
so the conclusion is unchanged.

---

## 4. Optimizations tried and rejected (all with measured numbers — don't retry them)

| # | Approach | Gain | Cost / why rejected |
|---|---|---|---|
| 1 | Chunked (grouped) layout with **per-slot explicit links inside the chunk** | **−55% (slower)** | the dependent load is still there, plus extra index arithmetic |
| 2 | Chunking with in-chunk order **implied by position** (= unrolled linked list) | iteration **8–13×** (measured 0.084–0.136 vs 1.10 ns/element) | insert/remove must move elements within the chunk ⇒ **indices stop being stable**; and you **don't have to write it** — see the end of §4 |
| 3 | Maintaining an adjacency bitmap (1 bit/slot: `next(i)==i+1`) | iteration **2.5×** | one extra store per link write ⇒ `churn` **2.80 → 5.66 ms (2× slower)** |
| 4 | Iterator-side lookahead (reading 8 `next` candidates in parallel when a segment runs out) | 1.6× in a standalone probe | measured in-crate at **2.40 vs 1.09 ns/element (2× slower)**, cause not identified ⇒ not shipped |
| 5 | Dropping the index to `u32` | **Implemented** (the `Ix` parameter; not the default, supplied by the `u32-index` feature): `append` **−37%** (T=8, 3.07 vs 4.86 ms), memory 24 → **16 B/slot**; `churn`/`iteration`/`middle_access` unchanged (±1%) | slot ceiling 2.1G (the top bit of `u32` is reserved for the free bit); one trap: a `{}`-argument ceiling `assert!` in `grow` drags the formatting machinery in, so the inliner gives up on `alloc_slot` ⇒ **churn +47%** (a `call` appears in the loop); moving it into a `#[cold]` helper took that back to zero |
| 6 | `reserve` before `append` | no measurable gain | std's `Vec::append` already reserves internally; `reserve_exact` is actually more expensive (3.9 → 7.5 ms) |
| 7 | `Nodes` field order (`data` centred / first) | within noise | see §2: the cache-line sets are the same |
| 8 | In-crate `madvise(MADV_HUGEPAGE)` | 8.2 → 1.1–1.4 ms at best | on this machine THP is in `madvise` mode with `nr_hugepages=0` and reruns are unstable (5860 page faults) ⇒ environment variable only |
| 9 | Making `clear` **scan `data` in memory order** and test `is_free` (no chain walk) | full table (no Drop glue) **7.1×** (1.08 → 0.15 ms) | **sparse (1M slots, 1000 live) 1.1 µs → 0.11 ms, 100× slower** (sparse `Nodes` with 64 B glue: 2 µs → 2.7 ms); full table + glue only breaks even (0.95–1.11×); the break-even point moves with width/layout (`u32` 0.25–0.50, `usize` index 0.50–0.75) ⇒ **adopt the hybrid: scan when `2 * len >= slots`** (threshold 0.5 = the upper bound of the break-even point across all six combinations, so no combination picks the slower path; the price is forgoing the 2–4× gain in the `u32` 0.25–0.5 band). Evidence: `src/tests.rs::probe_clear_vs_scan` |
| 10 | Marking a slot free by **writing only the byte holding the flag** (saving the load in the read-modify-write) | the emitted IR does save one load | on x86-64 `churn` is **5.5–7.5% slower** (Split/PackedLinks, four measurements, `VecDeque` reference flat, layout perturbation floor ±2%) ⇒ rejected; would need re-measuring on another ISA. **Note: the whole-function diff shows more than that one instruction changed** — register allocation, stack frame, instruction count and **loop alignment** all move ⇒ the ~7% cannot be attributed to a single instruction, and snippet-level assembly does not extrapolate (see the `FREE_BIT` documentation) |

---

### Is chunking worth writing yourself? No (a std combination suffices)

Assembling "chunking" directly out of std (`LinkedList<Vec<usize>>`, with a head offset inside the
chunk so both ends stay O(1)), K=64, compared against our `SplitList` on the same machine in the
same session:

| Metric (1M × `usize`) | this crate's `SplitList` | `LinkedList<Vec<usize>>` K=64 | Ratio |
|---|---|---|---|
| iteration | 1.08 ns/element | **0.112** | 9.6× |
| `at(N/2)` (walk to the middle) | 1.08 ns/element | **0.013** | 83× |
| `churn` (`pop_front`+`push_back`) | 2.81 ns/op | **1.70** | 1.65× |
| `end_ops` (push 1M + pop 1M, no preallocation) | 3.04 ns/op | **0.93** | 3.3× |
| memory/slot | 16 B | **~9 B** | 1.8× |
| element-level cursor / stable slot identity | ✓ | ✗ | — |

- "Chunking" is exactly **the unrolled-linked-list family ≈ `LinkedList<Vec<T>>`** (contiguous
  within a chunk, chained between chunks). That 8–13× on iteration is not the credit of some
  home-grown design — the std combination gets the same; using `VecDeque` as the chunk is actually
  worse (K=64: iteration 0.303 vs 0.094 for `Vec` chunks — a deque's contents can wrap around the
  ring, so the iterator cannot stream).
- And it **beats this crate on all four items above**: inside a chunk it is a bare array (one
  `Vec::push` / `head += 1`), while every end operation here touches 4–8 link fields (two neighbours
  plus the free-list push/pop) and memory is 1.8× higher (16 vs ~9 B/slot) ⇒ end-to-end we lose on
  **bytes per slot** and `churn` loses on **fields per operation**.
- What still differs here is only three things, all in **semantics** rather than throughput:
  ① **element-level cursors** (`at(pos)` / `insert_before` / `insert_after` / ghost position /
  circular movement); ② **stable slot identity** ("the index is the pointer" — which chunking
  precisely discards); ③ **zero allocation once warm** (one whole slab: `churn` allocates not a
  single byte; a chunked list `malloc`s/`free`s one block every K elements, ~2 ns amortized per
  element as measured here).
- ⇒ To get "chunked performance", don't stuff it into this crate: use `LinkedList<Vec<T>>`, or put
  chunks into this crate as elements (`List<Vec<T>, Split<Vec<T>>>` — chunk indices stay stable,
  within-chunk storage is contiguous, both ends O(1)).

## 5. Internal probes (explaining mechanisms, not the cost of public API calls)

| Item | Numbers | Source |
|---|---|---|
| `clear` vs "keep `pop`ping until empty" | 8 B **1.2×**, 64 B **2.9×**, 512 B **24.5×** (with Drop glue 64 B is only 1.6×) | `List::clear` docs |
| `clear`: chain walk vs slot scan (1M slots, `u32` index) | full table 1.08 → **0.15 ms (scan 7.1× faster)**, break-even at 0.25; sparse (1000 live) **1.1 µs → 0.11 ms (100× slower)**; full table + Drop glue break even (2.45 vs 2.57) ⇒ **hybrid: scan when `2 * len >= slots` (threshold 0.5)** | `List::clear` docs + `src/tests.rs::probe_clear_vs_scan` |
| Scanning threshold, by layout/width | break-even points: with `u32`, `Split` 0.25 / `PackedLinks` 0.25 / `Nodes` 0.50; with the `usize` index, 0.50 for all three layouts (≥1.5× needs 0.75). Narrowing the free-bit pass from 8 B to 4 B is what moved the break-even point down (0.50 → 0.25). **The hybrid threshold is 0.5**: at that density the six combinations measure 0.99–1.43× (break-even at worst; no combination is clearly slower) | same |
| Full-table gain of the scanning arm | quiet machine: `u32` 3.4–7.1×, `usize` 1.9–2.3×; with background load 1.65–2.96× — the scan is bandwidth-bound (see §1) while the chain-walk column is stable ⇒ **report ratios taken within a single run** | same |
| `Drop`: dropping in place along the live chain vs going through `clear` (1M, `u32`) | `usize` 1.098 ms → **1.9 µs** (the drop chain is eliminated); with 64 B glue **`Split` 2.498 → 1.889 ms (1.32×)**, `PackedLinks` 2.888 → 2.062 (1.40×), `Nodes` 3.569 → 2.069 (1.72×); the "drop only" column also frees the whole slot array, so the ratios are lower bounds | `List`'s `Drop` docs + the `clear_drop` group |
| Three-layer iteration breakdown (1M × `usize`, ns/element) | `iter()` 1.08 / walk the chain by hand 1.14 / sequential slot scan **0.062** (Split); Nodes 1.29 / 1.34 / 0.478 | §3.2 of this document |
| `append` byte accounting (mimalloc, 1M⊕1M) | ours 25.0 GB/s vs `Vec` 22.1 GB/s ⇒ **the whole gap is the number of bytes moved** (16 B/slot vs 8 B/slot, each moved twice because of growth) | §3.6 |
| Cost of index rewriting (`map(\|i\| i + base)`) | same speed as `extend_from_slice` (even faster) ⇒ **adding the offset is free** | §3.6 |
| Fixed size overhead | 16 B/slot (`u32`): 2× the payload at T=8, 12.5% at T=64 | §2 |

---

## 6. Selection guide

**Layout**

- Iteration-heavy (many reads, few writes) → `SplitList` / `PackedLinksList` (`Nodes` iterates ~22% slower at a 64 B payload).
- Frequent reads/writes at middle positions → `PackedLinksList` (`prev`/`next` share one `Link`, hence one cache line).
- Simplest allocation and memory / elements ≤ 48 B → `NodesList` (one `Vec`, one realloc).
- Elements ≥ 64 B and memory matters → `SplitList` / `PackedLinksList` (8 B less per slot).

**The two `append` APIs** (where this crate differs from std, whose `LinkedList::append` is an O(1) pointer splice)

- Target **has** free slots, or the source is sparse → `append_elementwise` (billed per live element; three orders of magnitude faster on a sparse source).
- Target must grow and the source is dense → `append` (a block move runs at bandwidth over bytes, independent of the element count).

**The two biggest knobs**

1. `with_capacity` / `reserve`: preallocate the target so it never grows ⇒ `append` **−50~60%**.
2. Reusing already-touched slots (`append_elementwise`): no fresh pages ⇒ orders of magnitude on a cold allocator.

**Environment (outside the crate, but the biggest influence on `append*`)**

```sh
cargo bench                                  # system malloc (default): watch page faults, not wall clock
cargo bench --features mimalloc              # mimalloc: the configuration behind most numbers here
GLIBC_TUNABLES=glibc.malloc.hugetlb=1 cargo bench   # glibc ≥ 2.35, route large blocks through huge pages
MIMALLOC_PURGE_DELAY=-1 cargo bench          # mimalloc: don't give pages back to the kernel
```

---

## 7. Reproduction

```sh
cargo bench                        # full suite; report in target/criterion/report/index.html
cargo bench -- 'FastList'          # only the fast-list comparison group (§9)
cargo bench -- 'clear_drop'        # clear/Drop comparison (§5)
cargo test --release -- --ignored --nocapture probe_clear_vs_scan   # clear's density curve and crossing point (§5)
cargo bench --features linked-list-cursors   # nightly: add the three std-cursor comparison rows back
cargo bench -- 'iteration|churn'   # filter (regex)
cargo test                         # correctness (20 tests, including per-slot invariant checks)
cargo miri test                    # strict provenance (the iterator goes through raw addresses)
```

`benches/list.rs` compiles on stable as well: the three comparison rows against std `LinkedList`
cursors (the `LinkedList` columns of `middle_insert_remove` / `random_remove_insert` /
`cursor_update`) need `#![feature(linked_list_cursors)]`, gated behind the
**`linked-list-cursors` feature** (**not in `default`**) ⇒ `cargo bench` works directly on stable,
and on nightly `cargo bench --features linked-list-cursors` adds those three rows. A feature rather
than a `build.rs` channel probe, because this is a library: `build.rs` would run for every
downstream user compiling this crate, while a feature is explicit, costs downstream nothing, and
matches `mimalloc`.

Most of the "internal probe" numbers in this document come from throwaway probes (`#[test]` +
`Instant`, min of 9 rounds) that were deleted after the run; the `clear` / `Drop` comparison
(`cargo bench -- 'clear_drop'`) is **permanent** as the `clear_drop` group: dense / sparse / refilled
after `clear` / drop, three layouts × no glue and 64 B Drop glue. The mechanisms and conclusions
live here and in the crate documentation (`List::clear`, `List`'s `Drop`, `Storage::append`,
`benches/list.rs`).

---

## 8. Four things unique beyond throughput

Throughput is not this crate's selling point (end of §4: chunking ≈ `LinkedList<Vec<T>>` is faster
on every item). What is genuinely unique is the four points below, the first two of which
**already have permanent tests** (read them as contracts).

1. **Stable slot identity, now a public handle ([`Slot`])**: insert/remove never moves other
   elements, and a slot stays put for the element's whole lifetime; `Storage::append` can even
   `+= base` a whole range of indices.
   - **Getting a handle**: `Cursor::slot` / `CursorMut::slot` / `List::front_slot` /
     `List::back_slot` / `List::iter_slots` (all `O(1)`); **using one**: `List::cursor_at` /
     `cursor_at_mut` / `remove_slot` / `move_to_front` / `move_to_back` (all `O(1)`, no logical
     position needed).
   - **Measured gain** (§3.7): the same work is **~8 ns/op** by handle and **~317–394 µs/op** by
     position — **about 40,000×** — the whole difference being "does it have to walk the chain to
     find it".
   - **"Is it live?" is decidable in `O(1)`**: the free bit lives in the **top bit of `prev`** and
     the test **never touches uninitialized `data`** (that would be UB). It is written once, when
     the slot becomes free (read-modify-write), and the link-write path has **no extra operation**;
     `append`'s bulk `+= base` **carries it along** (`2^63 | v` plus `base` is still
     `2^63 | (v + base)`).
   - Writing `prev = FREE_BIT` directly (to save a load) makes `churn` **7–8% slower** ⇒ the
     read-modify-write stays. Counter-intuitive, but reproducible.
   - Reading `prev` **without masking** (saving an `and` and shortening the address-generating
     dependency chain) is **neutral** (±1–3%, inconsistent direction, reference drifting along) ⇒
     adopted. The cost is making "**read `prev` only on live slots**" an explicit contract:
     `Storage::prev` carries a `debug_assert` (zero cost in release), and all 24 tests green show
     every existing path honours it. Measured cost: `churn` / `blob_end_ops` stay inside the noise
     ⇒ **free**.
   - **Trade-off: no generation check**. A handle answers only "is this slot live right now"; once
     the free list reuses a slot, an old handle points at the **new element** (classic ABA, pinned
     by a test). If you need stale handles to be detectable, use `slotmap` / `fast-list` (whose
     `LinkedListIndex` is a generation key, checked by `contains_key`) — at the cost of running
     1.7–3× slower on same-machine benchmarks.

2. **Zero allocation on the hot path**: once capacity is in place, `pop_front`/`push_back` never
   `malloc`s (test `churn_does_not_allocate`: 1M churns = **0** allocations across all three
   layouts; `LinkedList` in the same test does 1,000,000, and a chunked list with K=64 does
   31,250). With a fixed 16 B/slot and `with_capacity` ⇒ both memory and latency are predictable
   (after `clear`, slots go back onto the free list instead of to the kernel).

3. **Relocatable, pointer-free state (now a public API)**: the entire state of the chain = per slot
   `(T, prev, next)` plus `head`/`tail`/`free_head`/`free_tail`/`len` (test `state_is_relocatable`:
   moved verbatim into a new container, with iteration and invariants identical) ⇒ it can be
   serialized, placed in shared memory, or `mmap`ed with **no pointer fixup**. Entry points:
   [`List::into_raw`] / [`List::from_raw`] (with `RawList` accessors) plus each layout's
   `as_parts` / `into_parts` / `from_parts` (`Link`/`Node` are public) and the per-slot
   `iter_slots()` (live) / `free_slots()` (the free chain, LIFO). This is bought by the invariant
   "every value in the array is a valid index and `NIL` is never written into the arrays".

4. **Layout is a parameter**: `Split`/`PackedLinks`/`Nodes` share one implementation, and `Layout`
   (base + stride + offset) unifies all three into the raw addresses the iterator wants. That is
   both what makes the cross-layout benchmarks in this repository possible and what makes "change
   the layout" a one-type-parameter edit (measured trade-offs of the three layouts in §2, §3).

⇒ In one line: **this crate is not "the fastest linked list" but "the one you can use as the storage
engine of a `LinkedHashMap` / LRU while keeping an element-level list API, stable handles and
deterministic memory"**; for chunk-level throughput, use `LinkedList<Vec<T>>` (or put chunks in as
elements: `List<Vec<T>, Split<Vec<T>>>`).

---

## 9. Comparison with `fast-list` 0.1.8

The closest opponent in this niche: slotmap index + generation (`LinkedListIndex`). Every fact below
comes from both sides' source; the numbers are medians from one same-machine `cargo bench` (the
`FastList` series in `benches/list.rs`).

### 9.1 Performance

| Group (scale) | this crate's `SplitList` | `FastList` | Ratio | `LinkedList` | `VecDeque` |
|---|---|---|---|---|---|
| `end_ops/push_back_pop_front` (1M pushes + 1M pops) | **7.11 ms** | 12.54 ms | 1.8× | 5.37 ms | 2.58 ms |
| `iteration` (1M elements) | **1.21 ms** | 1.72 ms | 1.4× | 1.05 ms | 0.31 ms |
| `middle_access` (100 × N/2) | **54.4 ms** | 85.3 ms | 1.6× | 54.5 ms | 68 ns |
| `middle_insert_remove` (1000 insert/removes at N/2) | **561 µs** | 821 µs | 1.5× | 489 µs | 96.6 ms |
| `churn` (1M × pop+push) | **2.75 ms** | 5.09 ms | 1.9× | 6.15 ms | 1.68 ms |
| `random_remove_insert` (100 random positions) | **28.8 ms** | 43.3 ms | 1.5× | 29.8 ms | 4.67 ms |
| `slot_entry::by_handle` (100 handle entries) | **664 ns** | 43.5 ms | **60,000×** | — | — |
| `slot_entry::by_pos` (100 position entries) | **29.0 ms** | 43.8 ms | 1.5× | — | — |

Reading note: `end_ops` measures the allocator (the reading discipline in §1, ±20%); the other
groups ±5%.

**Why the `by_handle` cell differs by 60,000×** (a direct consequence of generation semantics, not
implementation quality):

- The benchmark body is "remove at a random position, insert back in place", and criterion **runs
  that loop repeatedly**.
- `fast-list`'s `LinkedListIndex` carries a generation: after a `remove` plus a re-insert, **the old
  handle is dead** (the new element gets a new generation). So after the first round every handle is
  invalid, and the only way out is, once `contains_key` reports death, **to find the element by
  position** (O(N)) — which is what the benchmark does, making `by_handle` ≈ `by_pos`.
- This crate's `Slot` is a raw index: after `remove` + `insert_before` the slot is reused LIFO,
  **so the old handle is still valid** and still points at that cell (ABA semantics), hence O(1).
  The price: **it will not tell you that the element has been swapped out**.

⇒ The difference between the two sides is not "who is faster" but **handle semantics after
mutation**: they guarantee "invalidation is detectable", we guarantee "identity never moves". To use
`fast-list` well on such workloads you must maintain an external "element → current handle" map
(updated on every insert/remove), whereas this crate does not need one (the price being ABA).

### 9.2 Features

| Dimension | this crate | `fast-list` 0.1.8 |
|---|---|---|
| Handle type | `Slot` (raw `usize` index, 8 B, no generation) | `LinkedListIndex` (slotmap key: `u32` index + `u32` generation, 8 B) |
| Stale handle / ABA | ✗ **undetectable**: after a slot is reused the old handle points at the new element (pinned by a test) | ✓ `contains_key()` validates immediately (a generation mismatch is `false`) |
| Handle → element | ✓ `cursor_at(_mut)` / `remove_slot`, O(1) | ✓ `get` / `get_mut` / `remove`, O(1) |
| Position → handle | ✓ `slot_at(pos)`, O(N) | ✓ `nth(pos)`, O(N) |
| **Element-level cursor** | ✓✓ `Cursor`/`CursorMut`: survives insert/remove, ghost position, `move_prev/next/steps`, `peek` neighbours | ✗ only `cursor_next/prev` (helpers for taking neighbours while traversing), no object that "stays on an element" |
| "Where was I" after a removal | ✓ the cursor stays in place, `insert_before` puts it back in the same spot | ✗ after `remove` you must use `LinkedListItem::next_index`/`prev_index` yourself as the anchor (which is what the benchmark does) |
| `move_to_front` / `move_to_back` | ✓ built in, O(1) | ✗ needs `insert_*` + `remove`, and **produces a new index ⇒ the old handle dies** |
| Bulk move `append` | ✓ O(1) slot renumbering plus a range-wide index offset | ✗ only `extend` (push one by one) |
| Attached data | ✗ none built in (but `Slot` is an ordinary index, usable directly as a key into any map/array) | ✓ `new_data::<V>()` / `new_data_sparse::<V>()` (slotmap's `SecondaryMap`, keyed by the handle) |
| Whole-chain state import/export | ✓ `into_raw` / `from_raw` + `as_parts` / `from_parts` + `iter_slots` / `free_slots` (can be persisted / shared in memory, with a round-trip test) | ✗ only copying element by element via `iter()` |
| Unordered traversal | ✓ `iter_slots() -> (Slot, &T)` | ✓ `iter_unordered() -> &LinkedListItem<T>` |
| Ordered traversal / `retain` / `split_off` | ✓ | ✓ |
| Find by **value** `contains` | ✓ | ✗ (only the by-handle `contains_key`) |
| Layout choice | ✓ three (Split / PackedLinks / Nodes) | ✗ one |
| Memory per slot (T=8) | **16 B** (data + prev + next, `u32` index, checked in §2) | `LinkedListItem<usize>` = **32 B** (value + index + next + prev), **plus** slotmap's per-slot version/occupancy metadata |
| Zero allocation on the hot path | ✓ `churn_does_not_allocate` | ✓ pinned in the same test (both sides do 0 mallocs) |
| Visible `unsafe` | the core is unchecked reads/writes plus a raw-address iterator, backed by miri and the full test suite | **0 occurrences** (the whole package's `unsafe` count is 0) ⇒ cheaper to audit/trust |
| Dependencies / `no_std` | none; turning off the `std` feature gives `no_std` + `alloc` | `slotmap` (+ optional `unstable` for `Walker`); uses only `core`, does not declare `no_std` |
| Derived impls | `Clone` / `Debug` / `PartialEq` / `Eq` / `Default` / `FromIterator` / `Extend` / `IntoIterator` | only `Debug` |

**Conclusion**: two trade-offs of the same data structure, not replacements for each other.

- Need a **generation check** (stale handles must be detectable), want slotmap's `SecondaryMap` to
  carry payload alongside, or want one less `unsafe` ⇒ use `fast-list`; the price is larger slots
  (32 B vs 24 B plus slotmap metadata), handles dying on every insert/remove, and no cursor /
  `move_to_*` / `append`.
- Need **cursor semantics** (staying in place after a removal, ghost position, relative movement),
  **three layouts**, **bulk moves**, **smaller slots**, **handles that survive mutation** ⇒ use this
  crate; the price is that stale handles are **undetectable** (bring your own generation counter,
  accept ABA semantics, or maintain an external map such as `new_data`).
