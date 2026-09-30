# `slot-list` performance summary

`List<T, S>` is a doubly linked list where **the index is the pointer**: nodes live in a
preallocated array of contiguous slots, and removed slots are threaded onto a free list for reuse.
The three memory layouts (`SplitList` / `PackedLinksList` / `NodesList`) are three `Storage`
parameters of the same generic type. The baselines are std's `LinkedList`, `VecDeque` and `Vec`.

> **Configuration of every number in this file**: default index width (`DefaultIx = usize`,
> 24 B/slot at T=8), `cargo bench --features mimalloc,linked-list-cursors`, criterion
> `sample_size(10)` / `measurement_time(1s)`, medians, single-threaded, one machine. The
> `u32-index` variant (16 B/slot) is compared in §4 row 5 and §5. Re-measured for 0.1.0.

---

## 0. Overview

| Question | Answer (full suite, 1M elements) |
|---|---|
| End operations `push`+`pop` (no preallocation) | 2.1–4.9× slower than `VecDeque`, roughly on par with `LinkedList` (5.1–10.0 vs 5.3–5.7 ms) — this group is dominated by **page faults and bytes copied** (§3.1) |
| Steady-state end operations `churn` (no allocation) | **2.0× faster than `LinkedList`** (2.77 vs 5.61 ms), 1.7× slower than `VecDeque` (1.65 ms) (§3.4) |
| Iteration | On par with `LinkedList` (1.11 vs 0.99 ms; ~1.1 ns/element = the dependent-load floor); **1.57× faster with a 64 B payload** (361 vs 568 µs) (§3.2) |
| Middle insert/remove | On par with `LinkedList` (538 vs 473 µs), ~150× faster than `VecDeque` (81.6 ms; it memmoves 500k elements every time) (§3.3) |
| Memory/slot | **24 B** (T=8, default `usize` index; the `u32` index is 16 B); all three layouts agree at 80 B for T=64 (§2) |
| Key differences from std `LinkedList` | ① no per-node allocation (why `churn` / `blob_end_ops` win); ② `append` is not an O(1) pointer splice (§3.6) |
| Confirmed at the ceiling | **within this crate's shape**: walking the chain during iteration and `append`'s copying. Three routes were implemented/modelled and rejected (§4). Going faster means changing the shape: chunking ≈ `LinkedList<Vec<T>>`, faster on every item but with no element-level cursor and no stable handle (end of §4) |
| Unique value (beyond performance) | **stable handle `Slot` (O(1) entry/remove/move, measured ~39,000× faster than going by position)**, zero allocation on the hot path (tested), relocatable state (tested), swappable layout — see §8 |
| Most effective knobs | reusing free slots (`append_elementwise`) and preallocating capacity (`with_capacity`), then the allocator / huge pages (§6) |

---

## 1. Method and reading discipline

- Single-threaded `cargo bench` (criterion, `sample_size(10)`, `measurement_time(1s)`, medians).
- **Index width**: every number below is the **default `usize`** (24 B/slot at T=8). The
  `u32-index` feature gives 16 B/slot; the `index_width` group and §4 row 5 compare the two
  directly. The benchmark prints the active width at startup.
- **Noise floor: ±5%** (less for small working sets such as `iteration` / `churn` / `cursor_update`);
  **±10–20%** for the large-allocation groups `append*` / `blob_*` — when the machine drifts, the
  same `churn` code moves from 2.80 to 3.01 ms while the `VecDeque` baseline does not budge, so
  **always read the baseline alongside**.
- **`end_ops` / `churn` / `append*` measure the allocator and the kernel, not the code**: the same
  `append` code swings from 2 ms to 16 ms depending on page faults. Compare **page faults inside
  the timing region**.
- Regression calls: run the two versions **alternating** (A/B/A/B), with an external baseline
  (`Vec`/`VecDeque`/`LinkedList`) as reference; only a difference beyond the noise floor counts.
- **Bandwidth-sensitive vs latency-sensitive**: purely bandwidth-bound items (`clear`'s scan arm,
  `append*`, sequential scans) can inflate **3–4×** on the same machine when something else is
  running, whereas latency-bound items (`churn`, cursors, chain-walking, which sits at ~1.1 ms)
  move only ~25% ⇒ **take the reference within the same run**; never divide two columns measured at
  different times. The sequential Vec scan alone moved between 0.062 and 0.108 ns/element across
  two runs of this document.
- Scales: `N` = 1M elements (`usize`, 8 B); `LARGE_N` = 250k (`Blob64`, 64 B).

---

## 2. The three layouts: memory and addresses (measured and checked)

`Layout` describes addresses as base + stride + offset (the index is a slot number):

```text
data address of element i = data base + i * data_stride + data_offset
prev value of element i   = *(prev base + i * prev_stride)
next value of element i   = *(next base + i * next_stride)
```

| Layout | Memory/slot (T=8 / T=64, `usize` index) | `data_stride` | `data_offset` | `prev`·`next` stride | Structure |
|---|---|---|---|---|---|
| `Split<T, I>` | **24 / 80** B | 8 / 64 | 0 | 8 | three separate `Vec`s |
| `PackedLinks<T, I>` | **24 / 80** B | 8 / 64 | 0 | 16 (`Link<I>`) | `data` interleaved with `Link{prev,next}` |
| `Nodes<T, I>` | **24 / 80** B | 24 / 80 | 0 | 24 / 80 | a single `Vec<Node<T, I>>` |

(Measured with `size_of` / `offset_of!`. With the `u32-index` feature the link is half as wide
again — 16 B/slot at T=8, 72 B at T=64, link strides 4 and 8. `data_offset` is 0 for all three
layouts at both widths.)

On the field order of `Node<T>` (`repr(Rust)`):

- The compiler **may reorder fields**. This repository measures it keeping the declaration order
  `[data][prev][next]` (all three fields are aligned ≤ 8 and no padding is available to save, so
  the order is stable), hence `data_offset` is 0.
- **The crate does not rely on this**: offsets always come from `offset_of!`. Moving `data` into
  the middle makes `data_offset` 8, and the whole suite still passes (the iterator goes through
  `Layout`).
- Both orders measured: the cache-line sets are **identical step by step** (`next` is always at
  `T+8`, `prev`/`data` merely swap) and the performance difference is inside the noise. The only
  risk is an **over-aligned** `T` (align ≥ 16) with `#[repr(C)]`, where centering `data` grows the
  node by 50% (32→48, 64→96); under `repr(Rust)` the compiler avoids this itself.

---

## 3. Full results

All tables below are 1M elements unless stated otherwise.

### 3.1 End operations `end_ops` (1M pushes + 1M pops)

The same `push_back`/`pop_front` and `push_front`/`pop_back` flows across five implementations:

| Shape (1M pushes + 1M pops, no preallocation) | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `push_back` + `pop_front` | 5.90 ms | 5.43 ms | 5.10 ms | 2.38 ms | 5.28 ms |
| `push_front` + `pop_back` | 9.99 ms | 5.83 ms | 5.76 ms | 2.03 ms | 5.67 ms |

This group does **not** preallocate, so it measures "grow-and-copy plus the first touch of every
page": 24 B/slot (against `VecDeque`'s 8 B) ⇒ three times the pages and the bytes copied, and that
is where `VecDeque`'s 2.0–2.4 ms advantage mostly comes from, not from faster code. The
`push_front`/`pop_back` column for `SplitList` (9.99 ms) is the widest outlier and sits at the edge
of the group's ±10–20% floor; the rest of the group is 5.1–5.9 ms, i.e. roughly `LinkedList`'s
level. Do not over-read the layout ordering here.

### 3.2 Iteration `iteration` / `blob_iter`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList | Vec |
|---|---|---|---|---|---|---|
| `iteration` (1M × 8 B, full traversal) | **1.11 ms** | 1.38 ms | 1.57 ms | 440.1 µs | 986.5 µs | 75.9 µs |
| `blob_iter` (250k × 64 B) | **360.9 µs** | 423.2 µs | 560.2 µs | 190.8 µs | 567.6 µs | 181.0 µs |

Splitting iteration into three layers (internal probe, min of 9 rounds, ns/element):

| Layout | public `iter()` | walking the chain by hand | sequential slot scan (no chain) |
|---|---|---|---|
| SplitList | 1.105 | 1.104 | 0.330 |
| PackedLinksList | 1.364 | 1.355 | 0.543 |
| NodesList | 1.333 | 1.344 | 0.420 |
| `Vec` sequential scan (reference) | — | — | 0.062 |

- **The iterator itself wastes nothing**: `iter()` is within noise of walking the chain by hand (a
  two-level check plus a counter are fully hidden behind the dependent load).
- **The expensive part is walking the chain**: ~1.1 ns/element is the **dependent-load latency** of
  "read `next` → go to that slot" (~4 cycles/element), not bandwidth. This is the inherent floor of
  a linked structure, and three improvement routes were tried (§4).
- **The sequential slot scan is bandwidth-bound, not free**: it still has to read the slot's link
  array to test liveness (Split: 16 B/element; PackedLinks/Nodes: 24 B) versus the `Vec`'s 8 B, so
  it lands at 0.33–0.54 ns/element, i.e. ~5–9× the `Vec` scan rather than at `Vec` speed. No public
  API scans all slots in slot order, so none of this is the cost of any call — it only bounds what
  the hybrid `clear` scan arm can reach (§4 row 9).
- **With a large payload it beats `LinkedList`**: `blob_iter` 360.9 µs vs 567.6 µs (1.57× faster) —
  `LinkedList` allocates each node separately, so every 64 B element costs a pointer chase and
  possibly a page fault, while our contiguous 80 B slots let the hardware prefetcher run. `Nodes`
  gains the least here (~55% slower than `Split`), because the whole node is pulled in per step.
- The numbers depend on **chain locality** (this chain is built with `push_back`, so it is
  0,1,2,… ascending and the prefetcher guesses right); after `random_remove_insert` the chain is
  fragmented and `iter()` drops to roughly one miss per step.

### 3.3 Locating and middle insert/remove `middle_access` / `middle_insert_remove`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `middle_access` (100 locates at N/2) | 53.37 ms | 54.93 ms | 64.85 ms | 56.8 ns | 50.79 ms |
| `middle_insert_remove` (1000 inserts + removes) | 537.7 µs | 555.8 µs | 646.3 µs | 81.63 ms | 473.0 µs |

`at(pos)` walks `O(min(pos, len-1-pos))` steps, so each "call" in `middle_access` actually walks
500k steps over a 1M-element chain ≈ 0.53 ms — about 1.1 ns per step, exactly matching the
chain-walk cost in §3.2. `VecDeque`'s 56 ns is an O(1) index; the two are not operations of the same
order, and the column is there only to mark the gap.

`middle_insert_remove` = insert + step + remove 1000 times in the middle of the chain, and **all
three layouts are on `LinkedList`'s scale** (within 1.4×): the bottleneck here is not the chain but
**allocating and freeing slots** (random accesses popping/pushing the free list), so the layout
difference — where each per-slot array sits — is nearly invisible. `VecDeque` needs 81.6 ms because
every middle insert is a memmove.

### 3.4 Steady state and perturbation `churn` / `random_remove_insert` / `cursor_update`

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `churn` (1M × pop_front+push_back) | 2.77 ms | 2.74 ms | 2.95 ms | 1.65 ms | 5.61 ms |
| `random_remove_insert` (100 random-position removes + inserts) | 28.33 ms | 31.61 ms | 34.30 ms | 4.39 ms | 27.27 ms |
| `cursor_update` (1M in-place read/writes at the chain midpoint) | 1.20 ms | 1.20 ms | 1.35 ms | 662.4 µs | 2.26 ms |

- `churn` **allocates nothing at all** (the list is built up front and slots cycle), so it measures
  **pure code**: each `pop_front`+`push_back` touches ~4–8 link fields (two neighbours plus the
  free list) while `VecDeque` touches only two indices ⇒ the whole 1.7× gap is here; and since
  `LinkedList` `malloc`s/`free`s every node, we are 2.0× faster — **the most direct payoff of
  "contiguous slots + free list"**.
- `random_remove_insert` fragments the chain and randomizes positions ⇒ all three layouts land at
  28–34 ms (`Split` best) while `VecDeque` needs only 4.4 ms — middle insert/remove is a memmove for
  it, whereas we must walk the chain (`at(pos)` is O(N)).
- `cursor_update` reads and writes a point in the middle of the chain in place: `Nodes` is slowest
  (1.35 ms, interleaved nodes force an extra hop to reach `data` each time), `LinkedList` 2.26 ms
  (one pointer chase per step).

### 3.5 Large payload `blob_end_ops` (`Blob64`, 64 B/element, 250k)

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `blob_end_ops` (250k × 64 B push + pop) | 3.93 ms | 3.50 ms | 3.75 ms | 2.86 ms | 4.16 ms |

With a large payload the link overhead amortizes: `VecDeque` leads by only 1.37× (vs 2.1–4.9× at
8 B), and **we beat `LinkedList`** (3.9 vs 4.2 ms), whose every 64 B node is a separate allocation.

80 B/slot = 64 B payload + 16 B links, so the payload is 80% of it. This is the archetypal "our link
overhead amortizes with a large payload" case: at 8 B elements, 24 B/slot is 3× the payload; at
64 B it is only 25% more.

### 3.6 `append`: two APIs (where this crate differs from std's semantics)

The two APIs **bill different units**; pick by shape:

- `append` (mirrors `LinkedList::append`): moves **all of the other list's slots** as one block
  (adding the offset to indices as it goes). It bills **the other list's slot count + your own
  storage size**, independent of the other side's live element count.
- `append_elementwise` (this crate's extension): moves element by element, preferring to fill your
  **existing free slots**. It bills **the live element count**.

`LinkedList::append` is an O(1) pointer splice (69.6 ns at 1M ⊕ 1M, independent of payload); we
gave that up for iteration locality (contiguous slots) — **a deliberate design choice**.

| Shape | SplitList | PackedLinksList | NodesList | VecDeque | Vec | LinkedList |
|---|---|---|---|---|---|---|
| `append` (1M ⊕ 1M, both sides must grow) | 4.08 ms | 3.62 ms | 3.42 ms | 1.17 ms | 946.6 µs | 69.6 ns |
| `append_blob` (250k ⊕ 250k, 64 B elements) | 2.64 ms | 2.79 ms | 2.71 ms | 2.00 ms | 1.85 ms | 44.6 ns |
| same, `::elementwise` (target has 250k free slots up front) | 1.77 ms | 1.85 ms | **1.65 ms** | — | — | — |

**Is a block `append` ~4× slower than `Vec::append`? It is billed per byte moved.** Each slot is
24 B (data + prev + next) where `Vec` moves 8 B, and because both sides are built by
`push_back`/`collect`, both reallocate — every byte is moved twice. The whole gap is that byte count,
not the index rewrite:

- **Index rewriting is free**: the `append` implementation adds the base to each moved index while
  copying it; measured against the `u32-index` variant, shrinking the links is what matters, not the
  add (§4 row 5).
- **Only two ways to close it** (both measured): drop the index to `u32` (the `u32-index` feature) ⇒
  the same `SplitList` append goes **3.67 → 2.48 ms (−32%)**; or preallocate the target so it never
  grows (see the selection guide, §6).
- **Large payload**: at 64 B the gap to `Vec` shrinks to 1.43× (2.64 vs 1.85 ms), because the extra
  16 B/slot is only 20% of an 80 B slot.
- **The cost of a block move is mostly "does the target need fresh pages"**: under mimalloc (these
  runs) 1M ⊕ 1M costs ~3.4–4.1 ms with zero page faults in the timing region; under the system
  allocator the same code is dominated by first-touch faults and can be several times slower. Always
  read the page-fault count (§1).

**Elementwise path, per element** (internal probe, min of 9 rounds; target has N live + N
already-touched free slots): **≈ 2.5 ns + 0.09 ns × payload bytes** — measured 2.52 ns at 8 B,
8.10 ns at 64 B, 24.5 ns at 256 B. Every element is moved twice (out of the source, into the
target), so this is not streaming bandwidth; a block move instead runs at bandwidth over bytes
regardless of element count. That is why the two paths converge as the payload grows, and why
`append_elementwise` wins outright when the target already has free slots (1.65–1.85 ms vs
2.64–2.79 ms for the block move at 64 B).

### 3.7 Handle entry vs position entry (`slot_entry` group)

**The same work, differing only in how the element is found**: 100 × "remove at a random position,
insert back in place" (`cursor_at(handle)` + `remove_current` + `insert_before` vs `at(pos)` + the
same two steps):

| Entry | SplitList | PackedLinksList | NodesList |
|---|---|---|---|
| by handle `cursor_at` (`O(1)`) | 719 ns | 611 ns | **547 ns** |
| by position `at(pos)` (walks the chain `O(min(pos, len-1-pos))`) | 28.33 ms | 31.60 ms | 34.26 ms |

(Units are totals for 100 operations; per operation: **~6–7 ns vs ~283–343 µs**, a gap of about
**39,000×**.) The by-handle column is the steady state where the 100 victim slots sit in cache (the
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
| 5 | Dropping the index to `u32` | **Implemented** (the `Ix` parameter; not the default, supplied by the `u32-index` feature). Same-run `index_width` group: `append` **3.67 → 2.48 ms (−32%)**, 24 → **16 B/slot**; `churn` 2.74 → 2.73 ms, `iteration` 1.16 → 1.10 ms, `middle_access` 53.31 → 53.24 ms (all within ±1%) | slot ceiling 2.1G (the top bit of `u32` is reserved for the free bit); one trap: a `{}`-argument ceiling `assert!` in `grow` drags the formatting machinery in, so the inliner gives up on `alloc_slot` ⇒ **churn +47%** (a `call` appears in the loop); moving it into a `#[cold]` helper took that back to zero |
| 6 | `reserve` before `append` | no measurable gain | std's `Vec::append` already reserves internally; `reserve_exact` is actually more expensive (3.9 → 7.5 ms) |
| 7 | `Nodes` field order (`data` centred / first) | within noise | see §2: the cache-line sets are the same |
| 8 | In-crate `madvise(MADV_HUGEPAGE)` | 8.2 → 1.1–1.4 ms at best | on this machine THP is in `madvise` mode with `nr_hugepages=0` and reruns are unstable (5860 page faults) ⇒ environment variable only |
| 9 | Making `clear` **scan `data` in memory order** and test `is_free` (no chain walk) | dense table (`usize` index, no glue): Split **1.083 → 0.706 ms (1.53×)**, PackedLinks **1.316 → 0.590 (2.23×)**, Nodes **1.392 → 0.705 (1.97×)**; with `u32` up to 3.0× | **sparse (0.001): walk ~0.001 ms vs scan 0.227–0.539 ms (200–400× slower)**; with 64 B Drop glue a dense table breaks even (0.89–1.08×) ⇒ **adopt the hybrid: scan when `2 * len >= slots`** (threshold 0.5). At density 0.5 the six layout×width combinations measure 0.99–1.81×, so no combination picks a slower path; the price is forgoing the larger scan gain in the 0.25–0.5 band. Evidence: `src/tests.rs::probe_clear_vs_scan` |
| 10 | Marking a slot free by **writing only the byte holding the flag** (saving the load in the read-modify-write) | the emitted IR does save one load | on x86-64 `churn` is **5.5–7.5% slower** (Split/PackedLinks, four measurements, `VecDeque` reference flat, layout perturbation floor ±2%) ⇒ rejected; would need re-measuring on another ISA. **Note: the whole-function diff shows more than that one instruction changed** — register allocation, stack frame, instruction count and **loop alignment** all move ⇒ the ~7% cannot be attributed to a single instruction, and snippet-level assembly does not extrapolate (see the `FREE_BIT` documentation) |

---

### Is chunking worth writing yourself? No (a std combination suffices)

Assembling "chunking" directly out of std (`LinkedList<Vec<usize>>`, with a head offset inside the
chunk so both ends stay O(1)), K=64, compared against our `SplitList` on the same machine in the
same session:

| Metric (1M × `usize`) | this crate's `SplitList` | `LinkedList<Vec<usize>>` K=64 | Ratio |
|---|---|---|---|
| iteration | 1.11 ns/element | **0.112** | 9.9× |
| `at(N/2)` (walk to the middle) | ~1.1 ns/element | **0.013** | ~85× |
| `churn` (`pop_front`+`push_back`) | 2.77 ns/op | **1.70** | 1.6× |
| `end_ops` (push 1M + pop 1M, no preallocation) | 5.90 ns/op | **0.93** | 6.3× |
| memory/slot | 24 B | **~9 B** | 2.7× |
| element-level cursor / stable slot identity | ✓ | ✗ | — |

- "Chunking" is exactly **the unrolled-linked-list family ≈ `LinkedList<Vec<T>>`** (contiguous
  within a chunk, chained between chunks). That 8–13× on iteration is not the credit of some
  home-grown design — the std combination gets the same; using `VecDeque` as the chunk is actually
  worse (K=64: iteration 0.303 vs 0.094 for `Vec` chunks — a deque's contents can wrap around the
  ring, so the iterator cannot stream).
- And it **beats this crate on all four items above**: inside a chunk it is a bare array (one
  `Vec::push` / `head += 1`), while every end operation here touches 4–8 link fields (two neighbours
  plus the free-list push/pop) and memory is 2.7× higher (24 vs ~9 B/slot) ⇒ end-to-end we lose on
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
| `clear`: chain walk vs slot scan (1M slots) | `usize` index, dense, no glue: Split 1.083 → **0.706 ms (1.53×)**, PackedLinks 1.316 → 0.590 (2.23×), Nodes 1.392 → 0.705 (1.97×); `u32`: 1.65× / 2.47× / 3.00×; sparse (0.001) walk ~0.001 ms vs scan 0.227–0.539 ms (200–400×); with 64 B Drop glue a dense table breaks even (0.89–1.08×) ⇒ **hybrid: scan when `2 * len >= slots` (threshold 0.5)** | `List::clear` docs + `src/tests.rs::probe_clear_vs_scan` |
| Scanning threshold, by layout/width | crossover (walk = scan): `usize` ≈ 0.50 for all three layouts; `u32` ≈ 0.25 (Nodes) to 0.50–0.75 (Split/PackedLinks). At the hybrid threshold 0.5 the six combinations measure 0.99–1.81× (worst case a tie; no combination measurably slower) | same |
| `clear` vs "keep `pop`ping until empty" | with Drop glue at 8 / 64 / 512 B the two are **within noise** (chain arm 0.96–1.04×, production `clear` 0.94–1.21×); without glue the element move is elided on both sides. `clear`'s real win is the single unlink pass and the hybrid scan arm, not a large multiple over `pop` | `List::clear` docs |
| `Drop`: dropping in place along the live chain vs going through `clear` | gluedrop vs gluedense: Split **1.95 vs 3.14 ms (1.61×)**, PackedLinks 2.26 vs 2.49 (1.10×), Nodes 2.14 vs 2.92 (1.36×); without glue the drop chain is eliminated (drop Split **1.76 µs** vs clear Split 640 µs) | `List`'s `Drop` docs + the `clear_drop` group |
| `clear_drop` group (1M slots) | dense clear Split/Packed/Nodes 640 / 596 / 843 µs; sparse 1.17 / 1.49 / 1.58 µs; refill after clear 2.35 / 1.83 / 1.77 ms | `clear_drop` group, §7 |
| Three-layer iteration breakdown (1M × `usize`, ns/element, min of 9) | `iter()` / walk by hand / sequential slot scan — Split 1.105 / 1.104 / 0.330; PackedLinks 1.364 / 1.355 / 0.543; Nodes 1.333 / 1.344 / 0.420; `Vec` scan reference 0.062 | §3.2 of this document |
| `append` per-byte billing | block `append` (1M ⊕ 1M) moves 24 B/slot against `Vec`'s 8 B, both sides reallocating ⇒ 4.08 vs 0.95 ms; `u32-index` cuts it to 2.48 ms (−32%) | §3.6, §4 row 5 |
| Elementwise per-element cost | ≈ 2.5 ns + 0.09 ns × payload bytes (2.52 / 8.10 / 24.5 ns at 8 / 64 / 256 B) | §3.6 |
| Fixed size overhead | 24 B/slot (`usize`): 3× the payload at T=8, 25% at T=64 | §2 |

---

## 6. Selection guide

**Layout**

- Iteration-heavy (many reads, few writes) → `SplitList` / `PackedLinksList` (`Nodes` iterates ~55% slower at a 64 B payload).
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
cargo bench                                     # system malloc: watch page faults, not wall clock
cargo bench --features mimalloc                 # mimalloc: the configuration behind most numbers here
GLIBC_TUNABLES=glibc.malloc.hugetlb=1 cargo bench   # glibc >= 2.35, route large blocks through huge pages
MIMALLOC_PURGE_DELAY=-1 cargo bench             # mimalloc: don't give pages back to the kernel
```

---

## 7. Reproduction

```sh
cargo bench --features mimalloc,linked-list-cursors   # the configuration of this document
cargo bench                                     # system malloc; page-fault accounting shows through
cargo bench -- 'FastList'                       # only the fast-list comparison group (§9)
cargo bench -- 'clear_drop'                     # clear/Drop comparison (§5)
cargo test --release -- --ignored --nocapture probe_clear_vs_scan   # clear's density curve and crossing point (§5)
cargo bench -- 'iteration|churn'                # filter (regex)
cargo test                                      # correctness (37 tests, including per-slot invariant checks)
cargo miri test                                 # strict provenance (the iterator goes through raw addresses)
```

`benches/list.rs` compiles on stable as well: the comparison rows against std `LinkedList` cursors
(the `LinkedList` columns of `middle_insert_remove` / `random_remove_insert` / `cursor_update`) need
`#![feature(linked_list_cursors)]`, gated behind the **`linked-list-cursors` feature** (**not in
`default`**) ⇒ `cargo bench` works directly on stable, and on nightly
`cargo bench --features linked-list-cursors` adds those three rows. A feature rather than a
`build.rs` channel probe, because this is a library: `build.rs` would run for every downstream user
compiling this crate, while a feature is explicit, costs downstream nothing, and matches `mimalloc`.

Most of the "internal probe" numbers in this document come from `#[test]` + `Instant` probes (min of
9 rounds); `clear`'s density curve is **permanent** as the ignored `probe_clear_vs_scan` test, and
the `clear` / `Drop` comparison is **permanent** as the `clear_drop` group: dense / sparse / refilled
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
   - **Measured gain** (§3.7): the same work is **~6–7 ns/op** by handle and **~283–343 µs/op** by
     position — **about 39,000×** — the whole difference being "does it have to walk the chain to
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
     `Storage::prev` carries a `debug_assert` (zero cost in release), and the whole suite green
     shows every existing path honours it. Measured cost: `churn` / `blob_end_ops` stay inside the
     noise ⇒ **free**.
   - **Trade-off: no generation check**. A handle answers only "is this slot live right now"; once
     the free list reuses a slot, an old handle points at the **new element** (classic ABA, pinned
     by a test). If you need stale handles to be detectable, use `slotmap` / `fast-list` (whose
     `LinkedListIndex` is a generation key, checked by `contains_key`) — at the cost of running
     1.5–1.9× slower on same-machine benchmarks (§9.1).

2. **Zero allocation on the hot path**: once capacity is in place, `pop_front`/`push_back` never
   `malloc`s (test `churn_does_not_allocate`: 1M churns = **0** allocations across all three
   layouts; `LinkedList` in the same test does 1,000,000, and a chunked list with K=64 does
   31,250). With a fixed 24 B/slot and `with_capacity` ⇒ both memory and latency are predictable
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
`FastList` series in `benches/list.rs`), same run and configuration as §3.

### 9.1 Performance

| Group (scale) | this crate's `SplitList` | `FastList` | Ratio | `LinkedList` | `VecDeque` |
|---|---|---|---|---|---|
| `end_ops/push_back_pop_front` (1M pushes + 1M pops) | **5.90 ms** | 11.28 ms | 1.9× | 5.28 ms | 2.38 ms |
| `iteration` (1M elements) | **1.11 ms** | 1.77 ms | 1.6× | 0.99 ms | 0.44 ms |
| `middle_access` (100 × N/2) | **53.37 ms** | 79.95 ms | 1.5× | 50.79 ms | 56.8 ns |
| `middle_insert_remove` (1000 insert/removes at N/2) | **538 µs** | 814 µs | 1.5× | 473 µs | 81.6 ms |
| `churn` (1M × pop+push) | **2.77 ms** | 5.10 ms | 1.8× | 5.61 ms | 1.65 ms |
| `random_remove_insert` (100 random positions) | **28.33 ms** | 42.39 ms | 1.5× | 27.27 ms | 4.39 ms |
| `cursor_update` (1M midpoint read/writes) | **1.20 ms** | 1.65 ms | 1.4× | 2.26 ms | 0.66 ms |
| `slot_entry::by_handle` (100 handle entries) | **719 ns** | 42.11 ms | **58,000×** | — | — |
| `slot_entry::by_pos` (100 position entries) | **28.33 ms** | 42.29 ms | 1.5× | — | — |

Reading note: `end_ops` measures the allocator (the reading discipline in §1, ±20%); the other
groups ±5%.

**Why the `by_handle` cell differs by ~58,000×** (a direct consequence of generation semantics, not
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
| Memory per slot (T=8) | **24 B** (data + prev + next, `usize` index, checked in §2; 16 B with `u32`) | `LinkedListItem<usize>` = **32 B** (value + index + next + prev), **plus** slotmap's per-slot version/occupancy metadata |
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
