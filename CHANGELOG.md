# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-09-30

Initial release.

### Added

- `List<T, S>`: a doubly linked list over a contiguous slot arena, with a LIFO
  free list and a slot index that is stable for the element's lifetime.
- Three storage layouts sharing one implementation: `Split`, `PackedLinks`, and
  `Nodes`, selected through the `Storage` trait.
- `SplitList`, `PackedLinksList`, and `NodesList` type aliases with a
  configurable link index width (`Ix`), plus the `u32-index` feature.
- Stable `Slot` handles: `front_slot` / `back_slot` / `iter_slots` / `free_slots`,
  and `cursor_at` / `cursor_at_mut` / `remove_slot` / `move_to_front` /
  `move_to_back` in `O(1)`.
- Element-level `Cursor` / `CursorMut`, aligned with
  `std::collections::LinkedList`, including ghost positions, wrap-around
  movement, `seek` / `move_steps`, and `is_head` / `is_tail`.
- `std`-style API and traits: `push_*` / `pop_*`, `front` / `back`,
  `retain`, `contains`, `append`, `split_off`, `remove`, `Clone`, `Debug`,
  `PartialEq`, `Eq`, `FromIterator`, `Extend`, and all three `IntoIterator` forms.
- `at(pos)` random access in `O(min(pos, len - 1 - pos))`.
- `append` (whole-slab move) and `append_elementwise` (reuse the target's free
  slots).
- `with_capacity` / `reserve` / `shrink_to_fit` for the slot array.
- Relocatable state: `into_raw` / `from_raw`, the per-layout
  `as_parts` / `into_parts` / `from_parts`, `RawList`, and the public `Link` / `Node`.
- `no_std` + `alloc` support behind the default `std` feature; zero dependencies.
