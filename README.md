# slot-list

A doubly linked list whose nodes all live in a single, preallocated, contiguous
slot array. The **slot index is the element's identity**: it stays stable for as
long as the element is alive, and freed slots are recycled through a LIFO free
list. That is where the name comes from.

- **No per-node allocation.** Growth is one `realloc` of the slot array; a warmed
  list performs zero allocations while pushing/popping.
- **Stable, `O(1)` handles.** `Slot` is a bare index you can store in a map or an
  adjacency list and use later to enter a cursor, remove, or move an element —
  no chain walk.
- **Element-level cursors** with the usual `std::collections::LinkedList`
  semantics (ghost positions, wrap-around movement, `insert_before`/`insert_after`).
- **Three memory layouts** behind one implementation: `Split`, `PackedLinks`, `Nodes`.
- **`no_std` + `alloc`, zero dependencies.**
- **Relocatable state.** The whole list is a slot array plus five integers, with
  no pointers — it can be serialized or placed in shared memory as-is.

The public API mirrors `std::collections::LinkedList` (`push_*`/`pop_*`,
`front`/`back`, cursors, iterators, `retain`, `append`, `split_off`, `Clone`,
`Debug`, `PartialEq`, `FromIterator`, `Extend`, …); differences are documented on
`List`.

## Quick start

```rust
use slot_list::{Slot, SplitList};

let mut list: SplitList<&str> = SplitList::new();
list.push_back("a");
list.push_back("b");
list.push_front("z");

assert_eq!(list.len(), 3);
assert_eq!(list.iter().copied().collect::<Vec<_>>(), ["z", "a", "b"]);

// A `Slot` is a stable handle: valid as long as the element is.
let handle: Slot = list.front_slot().unwrap();
assert!(list.cursor_at(handle).is_some());
```

Two notions of "position" are kept strictly apart (see the crate docs):

| | logical position `pos` | physical `slot` |
|---|---|---|
| meaning | ordinal on the chain | index in the slot array (= identity) |
| stable | changes on every insert/remove | stable for the element's lifetime |
| access | walk the chain, `O(min(pos, len-1-pos))` | `O(1)` handle |

## Layouts

All three layouts share one implementation; only where the fields sit differs.

| Alias | Layout | Arrangement |
|---|---|---|
| `SplitList<T>` | `Split` | three separate `Vec`s for `data` / `prev` / `next` |
| `PackedLinksList<T>` | `PackedLinks` | `data` in one `Vec`, `prev`+`next` packed into a `Link` array |
| `NodesList<T>` | `Nodes` | `data`+`prev`+`next` in a single `Node` array |

The second type parameter is the link index width (`u8`/`u16`/`u32`/`u64`/`usize`);
`u32` halves the link bytes per slot at the cost of a 2^31-1 slot ceiling.

## `no_std`

```text
cargo check --no-default-features     # no_std + alloc
```

The default `std` feature additionally enables the test suite and benchmarks.
The library itself needs only `core` and `alloc`.

## Performance

Measured results, mechanism explanations, the `append` v. `append_elementwise`
decision matrix, and the optimizations that were tried and rejected all live in
[`PERFORMANCE.md`](PERFORMANCE.md). Headline numbers (1M elements, `usize` index
— see the document for the reading discipline):

- steady-state `churn` is ~2.2x faster than `LinkedList`, and iteration is on par
  with it while being 1.74x faster at a 64-byte payload;
- a handle-based lookup is ~40000x faster than a position-based one on the same
  workload, because it skips the chain walk entirely.

Benchmarks reproduce with `cargo bench` (system allocator) or
`cargo bench --features mimalloc` (the configuration most numbers were taken in).

## Minimum supported Rust version

Rust **1.85** (edition 2024). The library builds on stable; the optional
`linked-list-cursors` benchmark feature requires nightly.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
