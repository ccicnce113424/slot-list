//! Cross-implementation benchmarks of three memory layouts (`SplitList` / `PackedLinksList` /
//! `NodesList`) against std `LinkedList`, `VecDeque`, `Vec`, and `fast-list`
//! (slotmap + generation handle).
//!
//! The three layouts are the same generic type `List<T, S>` with different `Storage`
//! parameters, so the benchmark bodies generate three instances from a macro instead of
//! three hand-written copies; the baselines are hand-written each.
//!
//! Run: `cargo bench [-- <filter regex>]`
//!
//! Compiles on stable: only the three rows that compare std `LinkedList` cursors
//! (`middle_insert_remove` / `random_remove_insert` / `cursor_update` for `LinkedList`) need
//! `#![feature(linked_list_cursors)]`, and they sit behind the `linked-list-cursors` feature,
//! which is not in `default`. So `cargo bench` works as-is on stable, and nightly users opt in
//! to those three rows explicitly:
//!
//! ```text
//! cargo bench                                                    # runs on any channel
//! cargo bench --features linked-list-cursors                     # nightly only (for the three cursor rows)
//! ```
//!
//! A feature instead of `build.rs` channel detection because this is a **library**: a
//! `build.rs` would run for every downstream user compiling this crate, whereas a feature is
//! explicit, costs downstream nothing, and matches what `mimalloc` does.
//! Report: `target/criterion/report/index.html`
//!
//! # Which group measures what
//!
//! | Group | What the timed region actually does |
//! |---|---|
//! | `churn`, `iteration`, `middle_access`, `middle_insert_remove`, `cursor_update`, `slot_entry`, `random_remove_insert`, `blob_iter` | our code. The list is built in setup and a removal recycles a free slot, so nothing enters the allocator and the numbers repeat |
//! | `end_ops_*`, `append`, `append_blob` | allocator and page faults. The timed region grows a fresh table or `Vec`, and the same code drifts from 2 ms to 16 ms as timed-region page faults go 0 to 8187, so compare by page-fault count |
//! | `clear_drop/drop_*` | deallocation of the backing vectors. The element-drop loop optimises away for a `usize` element |
//! | `clear_drop/gluedrop_*`, `gluedense_*` | `Drop` and `clear` with a 64 B element carrying real `Drop` glue, which is the pair that measures that code |
//! | `clear_drop/dense_*`, `sparse_*`, `refill_*` | `clear` at full and at sparse occupancy, and free-chain reuse |
//! | the `VecDeque` / `LinkedList` / `FastList` columns | the baseline, which for `churn` and `end_ops` mostly means the allocator |
//!
//! # Reading discipline
//!
//! Two traps first, because each of them yields a wrong number with no warning:
//!
//! - **criterion keys results by group and bench name.** A rename (the layouts were `SoaList` /
//!   `PackedList` / `AosList` before this file used `SplitList` / `PackedLinksList` /
//!   `NodesList`), or a run with a different feature set, leaves the old leaves on disk, and
//!   `base` / `change` then compare the fresh run against numbers from another day or another
//!   allocator. Delete `target/criterion` before a clean comparison, or give each run its own
//!   `--save-baseline <name>`.
//! - **`cargo bench` and `cargo bench --features mimalloc` are two different allocators**, and
//!   their results must not be mixed. Every number in `PERFORMANCE.md` is mimalloc.
//!
//! The noise floor, the alternating A/B protocol, the bandwidth-vs-latency rule and the
//! environment of the numbers in `PERFORMANCE.md` all live in `PERFORMANCE.md` §1.1 and §1.4.
//! Huge pages only come from environment variables (THP is in `madvise` mode here with
//! `nr_hugepages=0`, and the in-crate `madvise` is hit-or-miss):
//! `GLIBC_TUNABLES=glibc.malloc.hugetlb=1`, or mimalloc's page reuse / `MIMALLOC_PURGE_DELAY=-1`.
//! A shared CI runner is a different machine class; `.github/workflows/benches.yml` runs these
//! benches there and uploads `target/criterion` along with the `machine.txt` that names the CPU.
//!
//! # Where the numbers are
//!
//! `PERFORMANCE.md` at the repo root holds the memory/address tables for the three layouts, the
//! full result set, the selection matrix for the two `append` APIs, the handle-vs-position
//! comparison and the `fast-list` comparison. The measurements behind implementation choices
//! (`clear`'s scan threshold, the free-marking write, `u32`'s slot ceiling, the rejected
//! variants) are in the commit history and next to the code they explain.

#![cfg_attr(feature = "linked-list-cursors", feature(linked_list_cursors))]

// Global allocator. The system malloc by default; `cargo bench --features mimalloc` switches
// to mimalloc, to compare the effect of "does the allocator return memory to the kernel" on
// end operations.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use slot_list::{NodesList, PackedLinksList, Slot, SplitList};

use criterion::{BatchSize, Bencher, Criterion, criterion_group};

use std::{
    collections::{LinkedList, VecDeque},
    hint::black_box,
    time::Duration,
};

const N: usize = 1_000_000;

const LOOKUPS: usize = 100;

const MIDDLE_OPS: usize = 1_000;

const CHURN_OPS: usize = 1_000_000;

const RANDOM_OPS: usize = 100;

const CURSOR_UPDATE_OPS: usize = 1_000_000;

const LARGE_N: usize = 250_000;

// ============================================================
// Benchmark bodies for our three layouts
//
// The three layouts share one implementation (`List<T, S>`), so the bodies also generate
// three instances from a macro rather than three hand-written copies. The layout is only
// visible in `$ty`.
// ============================================================

macro_rules! bench_end_ops {
    ($name:ident, $ty:ty, $push:ident, $pop:ident) => {
        fn $name() {
            let mut list: $ty = <$ty>::new();

            for i in 0..N {
                list.$push(black_box(i));
            }

            while let Some(value) = list.$pop() {
                black_box(value);
            }
        }
    };
}

macro_rules! bench_construct {
    ($name:ident, $ty:ty) => {
        fn $name() -> $ty {
            let mut list: $ty = <$ty>::new();

            for i in 0..N {
                list.push_back(i);
            }

            list
        }
    };
}

macro_rules! bench_iter {
    ($name:ident, $ty:ty) => {
        fn $name(list: &$ty) {
            let mut sum = 0usize;

            for value in list.iter() {
                sum = sum.wrapping_add(*value);
            }

            black_box(sum);
        }
    };
}

macro_rules! bench_at_middle {
    ($name:ident, $ty:ty) => {
        fn $name(list: &mut $ty) {
            let mut sum = 0usize;
            let middle = N / 2;

            for _ in 0..LOOKUPS {
                let mut cursor = list.at(black_box(middle)).unwrap();

                sum = sum.wrapping_add(*cursor.current().unwrap());
            }

            black_box(sum);
        }
    };
}

macro_rules! bench_insert_before_remove {
    ($name:ident, $ty:ty) => {
        fn $name(list: &mut $ty) {
            let middle = N / 2;
            let mut cursor = list.at(middle).unwrap();

            for i in 0..MIDDLE_OPS {
                // Insert before the current position; the cursor does not move.
                cursor.insert_before(black_box(i));

                // Move to the just-inserted node.
                cursor.move_prev();

                // Remove it; the cursor returns to the original node.
                let value = cursor.remove_current().unwrap();

                black_box(value);
            }

            black_box(cursor.index());
        }
    };
}

/// Random position "remove one + insert back in place":
/// - `by_handle`: `cursor_at(handle)`, which is O(1);
/// - `by_pos`: `at(pos)`, which is O(N) and walks the chain.
///
/// Both do **exactly the same work**; only the entry differs.
macro_rules! bench_slot_vs_pos {
    ($by_handle:ident, $by_pos:ident, $ty:ty) => {
        fn $by_handle(list: &mut $ty, handles: &[Slot]) {
            for (i, &handle) in handles.iter().enumerate() {
                let mut cursor = list.cursor_at_mut(handle).unwrap();
                let value = cursor.remove_current().unwrap();

                cursor.insert_before(value ^ (i as usize));

                black_box(value);
            }
        }

        fn $by_pos(list: &mut $ty, positions: &[usize]) {
            for (i, &pos) in positions.iter().enumerate() {
                let mut cursor = list.at(pos).unwrap();
                let value = cursor.remove_current().unwrap();

                cursor.insert_before(value ^ (i as usize));

                black_box(value);
            }
        }
    };
}

macro_rules! bench_churn {
    ($name:ident, $ty:ty) => {
        fn $name(list: &mut $ty) {
            for i in 0..CHURN_OPS {
                let value = list.pop_front().unwrap();

                black_box(value);

                list.push_back(black_box(i));
            }
        }
    };
}

macro_rules! bench_random_remove_insert {
    ($name:ident, $ty:ty) => {
        fn $name(list: &mut $ty, positions: &[usize]) {
            for (i, &pos) in positions.iter().enumerate() {
                let mut cursor = list.at(pos).unwrap();

                let removed = cursor.remove_current().unwrap();

                black_box(removed);

                // The cursor now points at the next element (a ghost position when the
                // removed element was the tail; insert_before then appends to the end).
                cursor.insert_before(black_box(i));
            }

            black_box(list.len());
        }
    };
}

macro_rules! bench_cursor_update {
    ($name:ident, $ty:ty) => {
        fn $name(list: &mut $ty) {
            let mut cursor = list.at(N / 2).unwrap();

            let mut checksum = 0usize;

            for i in 0..CURSOR_UPDATE_OPS {
                let delta = black_box(i.wrapping_mul(0x9e37_79b9));

                let value = *cursor.current().unwrap();

                let new_value = value.wrapping_add(delta);

                *cursor.current().unwrap() = new_value;

                checksum ^= black_box(new_value);
            }

            black_box(checksum);
        }
    };
}

/// Whole-block transfer: `a.append(&mut b)` (inputs are built outside the timed region by
/// `iter_batched`).
///
/// **The return value must hand both lists back**: otherwise they would be dropped inside
/// the timed region, and dropping a 2M-node list would skew the benchmark completely (the
/// baselines' drop costs also differ widely: `LinkedList` is 2M `Box` deallocations, `Vec`
/// is zero). Returned to criterion, the drop happens after timing.
macro_rules! bench_append {
    ($name:ident, $ty:ty) => {
        fn $name(mut a: $ty, mut b: $ty) -> ($ty, $ty) {
            a.append(&mut b);

            black_box(a.len());

            (a, b)
        }
    };
}

macro_rules! bench_construct_blob {
    ($name:ident, $ty:ty) => {
        fn $name() -> $ty {
            let mut list: $ty = <$ty>::new();

            for i in 0..LARGE_N {
                list.push_back(Blob64::new(i));
            }

            list
        }
    };
}

macro_rules! bench_blob_iter {
    ($name:ident, $ty:ty) => {
        fn $name(list: &$ty) {
            let mut sum = 0usize;

            for value in list.iter() {
                sum = sum.wrapping_add(value.key());
            }

            black_box(sum);
        }
    };
}

macro_rules! bench_blob_end_ops {
    ($name:ident, $ty:ty) => {
        fn $name() {
            let mut list: $ty = <$ty>::new();

            for i in 0..LARGE_N {
                list.push_back(black_box(Blob64::new(i)));
            }

            while let Some(value) = list.pop_front() {
                black_box(value);
            }
        }
    };
}

// ---- Instantiations (three layouts x 12 benchmarks) ----

bench_end_ops!(
    soalist_push_back_pop_front,
    SplitList<usize>,
    push_back,
    pop_front
);
bench_end_ops!(
    packedlist_push_back_pop_front,
    PackedLinksList<usize>,
    push_back,
    pop_front
);
bench_end_ops!(
    aoslist_push_back_pop_front,
    NodesList<usize>,
    push_back,
    pop_front
);

bench_end_ops!(
    soalist_push_front_pop_back,
    SplitList<usize>,
    push_front,
    pop_back
);
bench_end_ops!(
    packedlist_push_front_pop_back,
    PackedLinksList<usize>,
    push_front,
    pop_back
);
bench_end_ops!(
    aoslist_push_front_pop_back,
    NodesList<usize>,
    push_front,
    pop_back
);

bench_construct!(make_soalist, SplitList<usize>);
bench_construct!(make_packedlist, PackedLinksList<usize>);
bench_construct!(make_aoslist, NodesList<usize>);

bench_iter!(soalist_iter, SplitList<usize>);
bench_iter!(packedlist_iter, PackedLinksList<usize>);
bench_iter!(aoslist_iter, NodesList<usize>);

bench_at_middle!(soalist_at_middle, SplitList<usize>);
bench_at_middle!(packedlist_at_middle, PackedLinksList<usize>);
bench_at_middle!(aoslist_at_middle, NodesList<usize>);

bench_insert_before_remove!(soalist_insert_before_remove, SplitList<usize>);
bench_insert_before_remove!(packedlist_insert_before_remove, PackedLinksList<usize>);
bench_insert_before_remove!(aoslist_insert_before_remove, NodesList<usize>);

bench_churn!(soalist_churn, SplitList<usize>);
bench_churn!(packedlist_churn, PackedLinksList<usize>);
bench_churn!(aoslist_churn, NodesList<usize>);

bench_random_remove_insert!(soalist_random_remove_insert, SplitList<usize>);
bench_random_remove_insert!(packedlist_random_remove_insert, PackedLinksList<usize>);
bench_random_remove_insert!(aoslist_random_remove_insert, NodesList<usize>);

bench_cursor_update!(soalist_cursor_update, SplitList<usize>);
bench_cursor_update!(packedlist_cursor_update, PackedLinksList<usize>);
bench_cursor_update!(aoslist_cursor_update, NodesList<usize>);

bench_append!(soalist_append, SplitList<usize>);
bench_append!(packedlist_append, PackedLinksList<usize>);
bench_append!(aoslist_append, NodesList<usize>);

// Large-payload (64 B) versions: data copying dominates, so layout differences show up.
bench_append!(soalist_blob_append, SplitList<Blob64>);
bench_append!(packedlist_blob_append, PackedLinksList<Blob64>);
bench_append!(aoslist_blob_append, NodesList<Blob64>);

bench_construct_blob!(make_soalist_blob, SplitList<Blob64>);
bench_construct_blob!(make_packedlist_blob, PackedLinksList<Blob64>);
bench_construct_blob!(make_aoslist_blob, NodesList<Blob64>);

bench_blob_iter!(soalist_blob_iter, SplitList<Blob64>);
bench_blob_iter!(packedlist_blob_iter, PackedLinksList<Blob64>);
bench_blob_iter!(aoslist_blob_iter, NodesList<Blob64>);

bench_blob_end_ops!(soalist_blob_push_back_pop_front, SplitList<Blob64>);
bench_blob_end_ops!(packedlist_blob_push_back_pop_front, PackedLinksList<Blob64>);
bench_blob_end_ops!(aoslist_blob_push_back_pop_front, NodesList<Blob64>);

// ============================================================
// Baselines: std VecDeque / std LinkedList / Vec
// ============================================================

// ---- Input generation and cursor positioning ----

fn next_rng(state: &mut u64) -> usize {
    // xorshift64
    let mut x = *state;

    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;

    *state = x;

    x as usize
}

fn make_random_positions(len: usize, count: usize) -> Vec<usize> {
    let mut state = 0x1234_5678_9abc_def0u64;

    let mut positions = Vec::with_capacity(count);

    for _ in 0..count {
        positions.push(next_rng(&mut state) % len);
    }

    positions
}

#[cfg(feature = "linked-list-cursors")]
fn linkedlist_cursor_at(
    list: &mut LinkedList<usize>,
    pos: usize,
) -> std::collections::linked_list::CursorMut<'_, usize> {
    let len = list.len();

    assert!(pos < len);

    if pos < len / 2 {
        let mut cursor = list.cursor_front_mut();

        for _ in 0..pos {
            cursor.move_next();
        }

        cursor
    } else {
        let mut cursor = list.cursor_back_mut();

        for _ in 0..(len - 1 - pos) {
            cursor.move_prev();
        }

        cursor
    }
}

// ---- End operations ----

fn vecdeque_push_back_pop_front() {
    let mut deque = VecDeque::new();

    for i in 0..N {
        deque.push_back(black_box(i));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn linkedlist_push_back_pop_front() {
    let mut list = LinkedList::new();

    for i in 0..N {
        list.push_back(black_box(i));
    }

    while let Some(value) = list.pop_front() {
        black_box(value);
    }
}

fn vecdeque_push_front_pop_back() {
    let mut deque = VecDeque::new();

    for i in 0..N {
        deque.push_front(black_box(i));
    }

    while let Some(value) = deque.pop_back() {
        black_box(value);
    }
}

fn linkedlist_push_front_pop_back() {
    let mut list = LinkedList::new();

    for i in 0..N {
        list.push_front(black_box(i));
    }

    while let Some(value) = list.pop_back() {
        black_box(value);
    }
}

// ---- Construction ----

fn make_vecdeque() -> VecDeque<usize> {
    let mut deque = VecDeque::new();

    for i in 0..N {
        deque.push_back(i);
    }

    deque
}

fn make_linkedlist() -> LinkedList<usize> {
    let mut list = LinkedList::new();

    for i in 0..N {
        list.push_back(i);
    }

    list
}

fn make_vec() -> Vec<usize> {
    (0..N).collect()
}

// ---- Pure iteration ----

fn vecdeque_iter(deque: &VecDeque<usize>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

fn linkedlist_iter(list: &LinkedList<usize>) {
    let mut sum = 0usize;

    for value in list.iter() {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

fn vec_iter(vec: &[usize]) {
    let mut sum = 0usize;

    for value in vec {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

// ---- Middle access ----

fn vecdeque_index_middle(deque: &VecDeque<usize>) {
    let mut sum = 0usize;

    let middle = N / 2;

    for _ in 0..LOOKUPS {
        sum = sum.wrapping_add(deque[black_box(middle)]);
    }

    black_box(sum);
}

fn linkedlist_at_middle(list: &LinkedList<usize>) {
    let mut sum = 0usize;

    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let value = list.iter().nth(black_box(middle)).unwrap();

        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

// ---- Local insert/remove at a known position ----

fn vecdeque_insert_remove(deque: &mut VecDeque<usize>) {
    let middle = N / 2;

    for i in 0..MIDDLE_OPS {
        deque.insert(middle, black_box(i));

        let value = deque.remove(middle).unwrap();

        black_box(value);
    }
}

#[cfg(feature = "linked-list-cursors")]
fn linkedlist_insert_before_remove(list: &mut LinkedList<usize>) {
    let mut cursor = linkedlist_cursor_at(list, N / 2);

    for i in 0..MIDDLE_OPS {
        cursor.insert_before(black_box(i));

        cursor.move_prev();

        let value = cursor.remove_current().unwrap();

        black_box(value);
    }

    black_box(cursor.index().unwrap());
}

// ---- Steady-state churn ----

fn vecdeque_churn(deque: &mut VecDeque<usize>) {
    for i in 0..CHURN_OPS {
        let value = deque.pop_front().unwrap();

        black_box(value);

        deque.push_back(black_box(i));
    }
}

fn linkedlist_churn(list: &mut LinkedList<usize>) {
    for i in 0..CHURN_OPS {
        let value = list.pop_front().unwrap();

        black_box(value);

        list.push_back(black_box(i));
    }
}

// ---- Random-position remove + insert ----

fn vecdeque_random_remove_insert(deque: &mut VecDeque<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let value = deque.remove(pos).unwrap();

        black_box(value);

        deque.insert(pos, black_box(i));
    }

    black_box(deque.len());
}

#[cfg(feature = "linked-list-cursors")]
fn linkedlist_random_remove_insert(list: &mut LinkedList<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let mut cursor = linkedlist_cursor_at(list, pos);

        let value = cursor.remove_current().unwrap();

        black_box(value);

        cursor.insert_before(black_box(i));
    }

    black_box(list.len());
}

// ---- Read/write at a known position ----

fn vecdeque_index_update(deque: &mut VecDeque<usize>) {
    let middle = N / 2;

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = deque[middle];

        let new_value = value.wrapping_add(delta);

        deque[middle] = new_value;

        checksum ^= black_box(new_value);
    }

    black_box(checksum);
}

#[cfg(feature = "linked-list-cursors")]
fn linkedlist_cursor_update(list: &mut LinkedList<usize>) {
    let mut cursor = linkedlist_cursor_at(list, N / 2);

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = *cursor.current().unwrap();

        let new_value = value.wrapping_add(delta);

        *cursor.current().unwrap() = new_value;

        checksum ^= black_box(new_value);
    }

    black_box(checksum);
}

// ---- append (whole-block transfer) ----

fn vecdeque_append(
    mut a: VecDeque<usize>,
    mut b: VecDeque<usize>,
) -> (VecDeque<usize>, VecDeque<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

/// std's `LinkedList::append` is an O(1) pointer splice (it moves no elements).
fn linkedlist_append(
    mut a: LinkedList<usize>,
    mut b: LinkedList<usize>,
) -> (LinkedList<usize>, LinkedList<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

/// `Vec` is the natural upper bound for a "whole-block memcpy"; it calibrates how expensive
/// "copy + fix indices" is.
fn vec_append(mut a: Vec<usize>, mut b: Vec<usize>) -> (Vec<usize>, Vec<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

// ============================================================
// 64-byte elements
// ============================================================

#[derive(Clone, Copy)]
struct Blob64([u64; 8]);

impl Blob64 {
    #[inline]
    fn new(value: usize) -> Self {
        Self([value as u64; 8])
    }

    #[inline]
    fn key(&self) -> usize {
        self.0[0] as usize
    }
}

// ---- 64B construction / iteration / end operations ----

fn make_vecdeque_blob() -> VecDeque<Blob64> {
    let mut deque = VecDeque::new();

    for i in 0..LARGE_N {
        deque.push_back(Blob64::new(i));
    }

    deque
}

fn make_linkedlist_blob() -> LinkedList<Blob64> {
    let mut list = LinkedList::new();

    for i in 0..LARGE_N {
        list.push_back(Blob64::new(i));
    }

    list
}

fn make_vec_blob() -> Vec<Blob64> {
    (0..LARGE_N).map(Blob64::new).collect()
}

// ---- 64B "roomy" constructors: for append_elementwise (LARGE_N live + LARGE_N free) ----

macro_rules! make_roomy_blob {
    ($name:ident, $ty:ty) => {
        fn $name() -> $ty {
            let mut list: $ty = <$ty>::new();

            for i in 0..2 * LARGE_N {
                list.push_back(Blob64::new(i));
            }

            for _ in 0..LARGE_N {
                list.pop_front();
            }

            list
        }
    };
}

make_roomy_blob!(make_soalist_blob_roomy, SplitList<Blob64>);
make_roomy_blob!(make_packedlist_blob_roomy, PackedLinksList<Blob64>);
make_roomy_blob!(make_aoslist_blob_roomy, NodesList<Blob64>);

// ---- 64B append (same shape as the usize version: both sides built by push/collect => the target must grow) ----

fn vecdeque_blob_append(
    mut a: VecDeque<Blob64>,
    mut b: VecDeque<Blob64>,
) -> (VecDeque<Blob64>, VecDeque<Blob64>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

fn linkedlist_blob_append(
    mut a: LinkedList<Blob64>,
    mut b: LinkedList<Blob64>,
) -> (LinkedList<Blob64>, LinkedList<Blob64>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

fn vec_blob_append(mut a: Vec<Blob64>, mut b: Vec<Blob64>) -> (Vec<Blob64>, Vec<Blob64>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

fn vecdeque_blob_iter(deque: &VecDeque<Blob64>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
}

fn linkedlist_blob_iter(list: &LinkedList<Blob64>) {
    let mut sum = 0usize;

    for value in list.iter() {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
}

fn vec_blob_iter(vec: &[Blob64]) {
    let mut sum = 0usize;

    for value in vec {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
}

fn vecdeque_blob_push_back_pop_front() {
    let mut deque = VecDeque::new();

    for i in 0..LARGE_N {
        deque.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn linkedlist_blob_push_back_pop_front() {
    let mut list = LinkedList::new();

    for i in 0..LARGE_N {
        list.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = list.pop_front() {
        black_box(value);
    }
}

bench_slot_vs_pos!(soalist_by_handle, soalist_by_pos, SplitList<usize>);
bench_slot_vs_pos!(
    packedlist_by_handle,
    packedlist_by_pos,
    PackedLinksList<usize>
);
bench_slot_vs_pos!(aoslist_by_handle, aoslist_by_pos, NodesList<usize>);

// ============================================================
// Comparison target: `fast-list` (slotmap index + generation)
//
// The API shape differs, so its bodies are written individually rather than from a macro:
// - the handle is a `LinkedListIndex` (a slotmap key carrying a generation; `contains_key`
//   is the ABA check);
// - `get`/`remove`/`insert_before` all take a handle; `nth(pos)` is an O(N) chain walk;
// - **no cursor**: after removing an element it cannot say "where I used to be", so here we
//   pick an anchor ourselves from `LinkedListItem::next_index`/`prev_index` to splice back
//   (on our side the cursor stays in place);
// - `iter()` yields `&LinkedListItem<T>`, with the value in `.value`;
// - no `append` / `move_to_front` / `move_to_back` (functional comparison in PERFORMANCE.md).
// ============================================================

use fast_list::{LinkedList as FastList, LinkedListIndex};

fn make_fastlist() -> FastList<usize> {
    let mut list: FastList<usize> = FastList::new();

    for i in 0..N {
        list.push_back(i);
    }

    list
}

fn fastlist_push_back_pop_front() {
    let mut list: FastList<usize> = FastList::new();

    for i in 0..N {
        list.push_back(black_box(i));
    }

    while let Some(value) = list.pop_front() {
        black_box(value);
    }
}

fn fastlist_iter(list: &FastList<usize>) {
    let mut sum = 0usize;

    for item in list.iter() {
        sum = sum.wrapping_add(item.value);
    }

    black_box(sum);
}

fn fastlist_at_middle(list: &FastList<usize>) {
    let mut sum = 0usize;
    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let index = list.nth(black_box(middle)).unwrap();

        sum = sum.wrapping_add(list.get(index).unwrap().value);
    }

    black_box(sum);
}

fn fastlist_insert_before_remove(list: &mut FastList<usize>) {
    let middle = list.nth(N / 2).unwrap();

    for i in 0..MIDDLE_OPS {
        let inserted = list.insert_before(black_box(middle), black_box(i));

        black_box(list.remove(inserted).unwrap().value);
    }
}

fn fastlist_churn(list: &mut FastList<usize>) {
    for i in 0..CHURN_OPS {
        let value = list.pop_front().unwrap();

        black_box(value);

        list.push_back(black_box(i));
    }
}

/// "Remove one + insert back in place": after the removal, use the successor it hands back
/// (or the predecessor when there is none) as the anchor to splice back in.
///
/// Note this **must not** `unwrap`: `fast-list` handles carry a generation, so after one
/// `remove` + re-insert the original handle is invalidated (the new element gets a new
/// generation). The benchmark caller checks with `contains_key` first.
fn fastlist_remove_reinsert(list: &mut FastList<usize>, handle: LinkedListIndex, i: usize) {
    let Some(item) = list.remove(black_box(handle)) else {
        return;
    };

    black_box(item.value);

    let new_value = black_box(item.value ^ i);

    match item.next_index.or(item.prev_index) {
        Some(anchor) => {
            list.insert_before(black_box(anchor), new_value);
        }
        None => {
            list.push_back(new_value);
        }
    }
}

/// Handle entry. **Every call must validate with `contains_key`**: the previous round's
/// "remove and re-insert" has already invalidated this batch of handles, and stale ones have
/// to be re-found by position. This step is the real cost of the generation semantics, not
/// something we added.
fn fastlist_by_handle(
    list: &mut FastList<usize>,
    handles: &[LinkedListIndex],
    positions: &[usize],
) {
    for (i, (&handle, &pos)) in handles.iter().zip(positions).enumerate() {
        let handle = if list.contains_key(handle) {
            handle
        } else {
            list.nth(black_box(pos)).unwrap()
        };

        fastlist_remove_reinsert(list, handle, i);
    }
}

fn fastlist_by_pos(list: &mut FastList<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let handle = list.nth(black_box(pos)).unwrap();

        fastlist_remove_reinsert(list, handle, i);
    }
}

// ============================================================
// Criterion groups
// ============================================================

// ============================================================
// Index-width comparison: `SplitList<usize, u32>` vs `SplitList<usize, usize>`
//
// Per-slot cost = `T` + two links => for `T = 8`, 16 B vs 24 B. Who benefits depends on the
// share of accesses to the link arrays: chain walks (`at(pos)`) and iteration gain most, end
// operations / handle entry least.
// ============================================================

bench_construct!(make_soa32, SplitList<usize, u32>);
bench_construct!(make_soa64, SplitList<usize, usize>);
bench_churn!(soa32_churn, SplitList<usize, u32>);
bench_churn!(soa64_churn, SplitList<usize, usize>);
bench_iter!(soa32_iter, SplitList<usize, u32>);
bench_iter!(soa64_iter, SplitList<usize, usize>);
bench_at_middle!(soa32_at_middle, SplitList<usize, u32>);
bench_at_middle!(soa64_at_middle, SplitList<usize, usize>);
bench_append!(soa32_append, SplitList<usize, u32>);
bench_append!(soa64_append, SplitList<usize, usize>);

fn index_width(c: &mut Criterion) {
    let mut soa32 = make_soa32();
    let mut soa64 = make_soa64();
    let mut g = c.benchmark_group("index_width");

    g.bench_function("churn/u32", |b| b.iter(|| soa32_churn(&mut soa32)));
    g.bench_function("churn/usize", |b| b.iter(|| soa64_churn(&mut soa64)));
    g.bench_function("iteration/u32", |b| b.iter(|| soa32_iter(&soa32)));
    g.bench_function("iteration/usize", |b| b.iter(|| soa64_iter(&soa64)));
    g.bench_function("middle_access/u32", |b| {
        b.iter(|| soa32_at_middle(&mut soa32))
    });
    g.bench_function("middle_access/usize", |b| {
        b.iter(|| soa64_at_middle(&mut soa64))
    });
    g.bench_function("append/u32", |b| {
        b.iter_batched(
            || (make_soa32(), make_soa32()),
            |(a, b)| soa32_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("append/usize", |b| {
        b.iter_batched(
            || (make_soa64(), make_soa64()),
            |(a, b)| soa64_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn end_ops(c: &mut Criterion) {
    let mut g = c.benchmark_group("end_ops/push_back_pop_front");
    g.bench_function("SplitList", |b| b.iter(soalist_push_back_pop_front));
    g.bench_function("PackedLinksList", |b| {
        b.iter(packedlist_push_back_pop_front)
    });
    g.bench_function("NodesList", |b| b.iter(aoslist_push_back_pop_front));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_push_back_pop_front));
    g.bench_function("LinkedList", |b| b.iter(linkedlist_push_back_pop_front));
    g.bench_function("FastList", |b| b.iter(fastlist_push_back_pop_front));
    g.finish();

    let mut g = c.benchmark_group("end_ops/push_front_pop_back");
    g.bench_function("SplitList", |b| b.iter(soalist_push_front_pop_back));
    g.bench_function("PackedLinksList", |b| {
        b.iter(packedlist_push_front_pop_back)
    });
    g.bench_function("NodesList", |b| b.iter(aoslist_push_front_pop_back));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_push_front_pop_back));
    g.bench_function("LinkedList", |b| b.iter(linkedlist_push_front_pop_back));
    g.finish();
}

fn iteration(c: &mut Criterion) {
    let splitlist = make_soalist();
    let packedlinkslist = make_packedlist();
    let nodeslist = make_aoslist();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let vec = make_vec();
    let fastlist = make_fastlist();

    let mut g = c.benchmark_group("iteration");
    g.bench_function("SplitList", |b| b.iter(|| soalist_iter(&splitlist)));
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_iter(&packedlinkslist))
    });
    g.bench_function("NodesList", |b| b.iter(|| aoslist_iter(&nodeslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_iter(&vecdeque)));
    g.bench_function("LinkedList", |b| b.iter(|| linkedlist_iter(&linkedlist)));
    g.bench_function("Vec", |b| b.iter(|| vec_iter(&vec)));
    g.bench_function("FastList", |b| b.iter(|| fastlist_iter(&fastlist)));
    g.finish();
}

fn middle_access(c: &mut Criterion) {
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let fastlist = make_fastlist();

    let mut g = c.benchmark_group("middle_access");
    g.bench_function("SplitList", |b| {
        b.iter(|| soalist_at_middle(&mut splitlist))
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_at_middle(&mut packedlinkslist))
    });
    g.bench_function("NodesList", |b| {
        b.iter(|| aoslist_at_middle(&mut nodeslist))
    });
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_index_middle(&vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_at_middle(&linkedlist))
    });
    g.bench_function("FastList", |b| b.iter(|| fastlist_at_middle(&fastlist)));
    g.finish();
}

fn middle_insert_remove(c: &mut Criterion) {
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

    let mut g = c.benchmark_group("middle_insert_remove");
    g.bench_function("SplitList", |b| {
        b.iter(|| soalist_insert_before_remove(&mut splitlist))
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_insert_before_remove(&mut packedlinkslist))
    });
    g.bench_function("NodesList", |b| {
        b.iter(|| aoslist_insert_before_remove(&mut nodeslist))
    });
    g.bench_function("VecDeque", |b| {
        b.iter(|| vecdeque_insert_remove(&mut vecdeque))
    });
    #[cfg(feature = "linked-list-cursors")]
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_insert_before_remove(&mut linkedlist))
    });
    g.bench_function("FastList", |b| {
        b.iter(|| fastlist_insert_before_remove(&mut fastlist))
    });
    g.finish();
}

fn churn(c: &mut Criterion) {
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

    let mut g = c.benchmark_group("churn");
    g.bench_function("SplitList", |b| b.iter(|| soalist_churn(&mut splitlist)));
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_churn(&mut packedlinkslist))
    });
    g.bench_function("NodesList", |b| b.iter(|| aoslist_churn(&mut nodeslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_churn(&mut vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_churn(&mut linkedlist))
    });
    g.bench_function("FastList", |b| b.iter(|| fastlist_churn(&mut fastlist)));
    g.finish();
}

fn random_remove_insert(c: &mut Criterion) {
    let positions = make_random_positions(N, RANDOM_OPS);
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();
    let fast_handles: Vec<LinkedListIndex> = positions
        .iter()
        .map(|&pos| fastlist.nth(pos).unwrap())
        .collect();

    let mut g = c.benchmark_group("random_remove_insert");
    g.bench_function("SplitList", |b| {
        b.iter(|| soalist_random_remove_insert(&mut splitlist, &positions))
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_random_remove_insert(&mut packedlinkslist, &positions))
    });
    g.bench_function("NodesList", |b| {
        b.iter(|| aoslist_random_remove_insert(&mut nodeslist, &positions))
    });
    g.bench_function("VecDeque", |b| {
        b.iter(|| vecdeque_random_remove_insert(&mut vecdeque, &positions))
    });
    #[cfg(feature = "linked-list-cursors")]
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_random_remove_insert(&mut linkedlist, &positions))
    });
    g.bench_function("FastList::by_handle", |b| {
        b.iter(|| fastlist_by_handle(&mut fastlist, &fast_handles, &positions))
    });
    g.bench_function("FastList::by_pos", |b| {
        b.iter(|| fastlist_by_pos(&mut fastlist, &positions))
    });
    g.finish();
}

fn cursor_update(c: &mut Criterion) {
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();

    let mut g = c.benchmark_group("cursor_update");
    g.bench_function("SplitList", |b| {
        b.iter(|| soalist_cursor_update(&mut splitlist))
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_cursor_update(&mut packedlinkslist))
    });
    g.bench_function("NodesList", |b| {
        b.iter(|| aoslist_cursor_update(&mut nodeslist))
    });
    g.bench_function("VecDeque", |b| {
        b.iter(|| vecdeque_index_update(&mut vecdeque))
    });
    #[cfg(feature = "linked-list-cursors")]
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_cursor_update(&mut linkedlist))
    });
    g.finish();
}

fn append(c: &mut Criterion) {
    let mut g = c.benchmark_group("append");

    g.bench_function("SplitList", |b| {
        b.iter_batched(
            || (make_soalist(), make_soalist()),
            |(a, b)| soalist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter_batched(
            || (make_packedlist(), make_packedlist()),
            |(a, b)| packedlist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("NodesList", |b| {
        b.iter_batched(
            || (make_aoslist(), make_aoslist()),
            |(a, b)| aoslist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("VecDeque", |b| {
        b.iter_batched(
            || (make_vecdeque(), make_vecdeque()),
            |(a, b)| vecdeque_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("LinkedList", |b| {
        b.iter_batched(
            || (make_linkedlist(), make_linkedlist()),
            |(a, b)| linkedlist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("Vec", |b| {
        b.iter_batched(
            || (make_vec(), make_vec()),
            |(a, b)| vec_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn append_blob(c: &mut Criterion) {
    let mut g = c.benchmark_group("append_blob");

    g.bench_function("SplitList", |b| {
        b.iter_batched(
            || (make_soalist_blob(), make_soalist_blob()),
            |(a, b)| soalist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("PackedLinksList", |b| {
        b.iter_batched(
            || (make_packedlist_blob(), make_packedlist_blob()),
            |(a, b)| packedlist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("NodesList", |b| {
        b.iter_batched(
            || (make_aoslist_blob(), make_aoslist_blob()),
            |(a, b)| aoslist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    // Elementwise path: the target has LARGE_N free slots up front (its recommended usage)
    g.bench_function("SplitList::elementwise", |b| {
        b.iter_batched(
            || (make_soalist_blob_roomy(), make_soalist_blob()),
            |(mut a, mut b)| {
                a.append_elementwise(&mut b);
                black_box(a.len());
                (a, b)
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("PackedLinksList::elementwise", |b| {
        b.iter_batched(
            || (make_packedlist_blob_roomy(), make_packedlist_blob()),
            |(mut a, mut b)| {
                a.append_elementwise(&mut b);
                black_box(a.len());
                (a, b)
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("NodesList::elementwise", |b| {
        b.iter_batched(
            || (make_aoslist_blob_roomy(), make_aoslist_blob()),
            |(mut a, mut b)| {
                a.append_elementwise(&mut b);
                black_box(a.len());
                (a, b)
            },
            BatchSize::PerIteration,
        )
    });
    g.bench_function("VecDeque", |b| {
        b.iter_batched(
            || (make_vecdeque_blob(), make_vecdeque_blob()),
            |(a, b)| vecdeque_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("LinkedList", |b| {
        b.iter_batched(
            || (make_linkedlist_blob(), make_linkedlist_blob()),
            |(a, b)| linkedlist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("Vec", |b| {
        b.iter_batched(
            || (make_vec_blob(), make_vec_blob()),
            |(a, b)| vec_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.finish();
}

fn blob_iter(c: &mut Criterion) {
    let splitlist = make_soalist_blob();
    let packedlinkslist = make_packedlist_blob();
    let nodeslist = make_aoslist_blob();
    let vecdeque = make_vecdeque_blob();
    let linkedlist = make_linkedlist_blob();
    let vec = make_vec_blob();

    let mut g = c.benchmark_group("blob_iter");
    g.bench_function("SplitList", |b| b.iter(|| soalist_blob_iter(&splitlist)));
    g.bench_function("PackedLinksList", |b| {
        b.iter(|| packedlist_blob_iter(&packedlinkslist))
    });
    g.bench_function("NodesList", |b| b.iter(|| aoslist_blob_iter(&nodeslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_blob_iter(&vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_blob_iter(&linkedlist))
    });
    g.bench_function("Vec", |b| b.iter(|| vec_blob_iter(&vec)));
    g.finish();
}

/// Handle entry (O(1)) vs logical-position entry (O(N)): **the same work**, only the entry
/// differs. This group is the quantitative basis for "should `Slot` be exposed".
fn slot_entry(c: &mut Criterion) {
    let positions = make_random_positions(N, RANDOM_OPS);
    let mut splitlist = make_soalist();
    let mut packedlinkslist = make_packedlist();
    let mut nodeslist = make_aoslist();

    // Handles are fetched outside the timed region (fetching means walking to that position, O(N))
    let soa_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| splitlist.at(pos).unwrap().slot().unwrap())
        .collect();
    let packed_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| packedlinkslist.at(pos).unwrap().slot().unwrap())
        .collect();
    let aos_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| nodeslist.at(pos).unwrap().slot().unwrap())
        .collect();
    let mut fastlist = make_fastlist();
    let fast_handles: Vec<LinkedListIndex> = positions
        .iter()
        .map(|&pos| fastlist.nth(pos).unwrap())
        .collect();

    let mut g = c.benchmark_group("slot_entry");
    g.bench_function("SplitList::by_handle", |b| {
        b.iter(|| soalist_by_handle(&mut splitlist, &soa_handles))
    });
    g.bench_function("SplitList::by_pos", |b| {
        b.iter(|| soalist_by_pos(&mut splitlist, &positions))
    });
    g.bench_function("PackedLinksList::by_handle", |b| {
        b.iter(|| packedlist_by_handle(&mut packedlinkslist, &packed_handles))
    });
    g.bench_function("PackedLinksList::by_pos", |b| {
        b.iter(|| packedlist_by_pos(&mut packedlinkslist, &positions))
    });
    g.bench_function("NodesList::by_handle", |b| {
        b.iter(|| aoslist_by_handle(&mut nodeslist, &aos_handles))
    });
    g.bench_function("NodesList::by_pos", |b| {
        b.iter(|| aoslist_by_pos(&mut nodeslist, &positions))
    });
    g.bench_function("FastList::by_handle", |b| {
        b.iter(|| fastlist_by_handle(&mut fastlist, &fast_handles, &positions))
    });
    g.bench_function("FastList::by_pos", |b| {
        b.iter(|| fastlist_by_pos(&mut fastlist, &positions))
    });
    g.finish();
}

fn blob_end_ops(c: &mut Criterion) {
    let mut g = c.benchmark_group("blob_end_ops");
    g.bench_function("SplitList", |b| b.iter(soalist_blob_push_back_pop_front));
    g.bench_function("PackedLinksList", |b| {
        b.iter(packedlist_blob_push_back_pop_front)
    });
    g.bench_function("NodesList", |b| b.iter(aoslist_blob_push_back_pop_front));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_blob_push_back_pop_front));
    g.bench_function("LinkedList", |b| {
        b.iter(linkedlist_blob_push_back_pop_front)
    });
    g.finish();
}

// ============================================================
// clear / Drop probes
//
// `iter_batched` keeps construction and destruction out of the timed region, so each row
// isolates one call:
//
// - `dense_*` and `sparse_*`: `clear` at full occupancy and at 0.1%. `clear` is hybrid
//   (`2 * len >= slots` scans `data` in memory order, otherwise it walks the free chain), so
//   the dense rows measure the scan and the sparse rows the walk. The two cost models are
//   unrelated: the walk is O(live) random slot accesses, the scan is O(slots) sequential
//   accesses.
// - `refill_*`: pushing N elements back into the list just cleared. The order of the free chain
//   decides reuse locality, and nothing in the timed region enters the allocator.
// - `drop_*` and `gluedrop_*`: destroying the list. With a `usize` element the element-drop loop
//   optimises away, so `drop_*` only deallocates the backing vectors; `gluedrop_*` uses a 64 B
//   element with real `Drop` glue and is the row to read for `Drop` code.
// - `gluedense_*`: `clear` with that same 64 B element.
//
// Numbers and the density curve are in the commit messages and in
// `src/tests.rs::probe_clear_vs_scan`. The scan arm needs private `List` fields, so it cannot
// run from this file.
// ============================================================
macro_rules! bench_clear_drop {
    ($ty:ty, $maker:ident, $dense:ident, $sparse:ident, $refill:ident, $dropper:ident) => {
        /// Dense: `slots == len == N`.
        fn $dense(b: &mut Bencher) {
            b.iter_batched(
                || $maker(),
                |mut list| {
                    list.clear();

                    list
                },
                BatchSize::PerIteration,
            );
        }

        /// Sparse: only 1000 of 1M slots are live (999_000 sit on the free chain).
        fn $sparse(b: &mut Bencher) {
            b.iter_batched(
                || {
                    let mut list = $maker();

                    while list.len() > 1000 {
                        list.pop_front();
                    }

                    list
                },
                |mut list| {
                    list.clear();

                    list
                },
                BatchSize::PerIteration,
            );
        }

        /// Refill after `clear`: the **order** of the free chain determines reuse locality.
        /// Everything moves within already-allocated slots; the allocator is never entered.
        fn $refill(b: &mut Bencher) {
            b.iter_batched(
                || {
                    let mut list = $maker();

                    list.clear();

                    list
                },
                |mut list| {
                    for i in 0..N {
                        list.push_back(black_box(i));
                    }

                    list
                },
                BatchSize::PerIteration,
            );
        }

        /// Destroy the whole list: ideal cost = O(live) in-place drops, no free-chain
        /// maintenance.
        fn $dropper(b: &mut Bencher) {
            b.iter_batched(|| $maker(), drop, BatchSize::PerIteration);
        }
    };
}

bench_clear_drop!(
    SplitList,
    make_soalist,
    soalist_clear_dense,
    soalist_clear_sparse,
    soalist_refill,
    soalist_drop
);
bench_clear_drop!(
    PackedLinksList,
    make_packedlist,
    packedlist_clear_dense,
    packedlist_clear_sparse,
    packedlist_refill,
    packedlist_drop
);
bench_clear_drop!(
    NodesList,
    make_aoslist,
    aoslist_clear_dense,
    aoslist_clear_sparse,
    aoslist_refill,
    aoslist_drop
);

/// 64 B payload with Drop glue.
///
/// `usize` cannot reveal the real difference in `Drop`/`clear`: without glue the whole drop
/// chain is elided by LLVM. Here `drop` actually reads a field to pin the destruction down.
struct Drop64([u64; 8]);

impl Drop64 {
    #[inline]
    fn new(value: usize) -> Self {
        Self([value as u64; 8])
    }
}

impl Drop for Drop64 {
    #[inline]
    fn drop(&mut self) {
        black_box(self.0[0]);
    }
}

macro_rules! make_drop64_list {
    ($name:ident, $ty:ty) => {
        fn $name() -> $ty {
            let mut list: $ty = <$ty>::new();

            for i in 0..N {
                list.push_back(Drop64::new(i));
            }

            list
        }
    };
}

make_drop64_list!(make_soalist_drop64, SplitList<Drop64>);
make_drop64_list!(make_packedlist_drop64, PackedLinksList<Drop64>);
make_drop64_list!(make_aoslist_drop64, NodesList<Drop64>);

macro_rules! bench_clear_drop_glue {
    ($ty:ty, $maker:ident, $dense:ident, $dropper:ident) => {
        fn $dense(b: &mut Bencher) {
            b.iter_batched(
                || $maker(),
                |mut list| {
                    list.clear();

                    list
                },
                BatchSize::PerIteration,
            );
        }

        fn $dropper(b: &mut Bencher) {
            b.iter_batched(|| $maker(), drop, BatchSize::PerIteration);
        }
    };
}

bench_clear_drop_glue!(
    SplitList<Drop64>,
    make_soalist_drop64,
    soalist_glue_clear_dense,
    soalist_glue_drop
);
bench_clear_drop_glue!(
    PackedLinksList<Drop64>,
    make_packedlist_drop64,
    packedlist_glue_clear_dense,
    packedlist_glue_drop
);
bench_clear_drop_glue!(
    NodesList<Drop64>,
    make_aoslist_drop64,
    aoslist_glue_clear_dense,
    aoslist_glue_drop
);

fn clear_drop(c: &mut Criterion) {
    let mut g = c.benchmark_group("clear_drop");

    g.bench_function("dense/SplitList", soalist_clear_dense);
    g.bench_function("dense/PackedLinksList", packedlist_clear_dense);
    g.bench_function("dense/NodesList", aoslist_clear_dense);
    g.bench_function("sparse/SplitList", soalist_clear_sparse);
    g.bench_function("sparse/PackedLinksList", packedlist_clear_sparse);
    g.bench_function("sparse/NodesList", aoslist_clear_sparse);
    g.bench_function("refill/SplitList", soalist_refill);
    g.bench_function("refill/PackedLinksList", packedlist_refill);
    g.bench_function("refill/NodesList", aoslist_refill);
    g.bench_function("drop/SplitList", soalist_drop);
    g.bench_function("drop/PackedLinksList", packedlist_drop);
    g.bench_function("drop/NodesList", aoslist_drop);
    g.bench_function("gluedense/SplitList", soalist_glue_clear_dense);
    g.bench_function("gluedense/PackedLinksList", packedlist_glue_clear_dense);
    g.bench_function("gluedense/NodesList", aoslist_glue_clear_dense);
    g.bench_function("gluedrop/SplitList", soalist_glue_drop);
    g.bench_function("gluedrop/PackedLinksList", packedlist_glue_drop);
    g.bench_function("gluedrop/NodesList", aoslist_glue_drop);

    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(1));
    targets =
        end_ops,
        iteration,
        middle_access,
        middle_insert_remove,
        churn,
        random_remove_insert,
        cursor_update,
        append,
        append_blob,
        blob_iter,
        blob_end_ops,
        slot_entry,
        clear_drop,
        index_width,
}

fn main() {
    // Numbers must travel with the configuration: index width determines bytes per slot
    // (16 vs 24 B) and the trend of the serial-bandwidth metrics.
    println!(
        "Index width: DefaultIx = u{} ({} bytes); `u32-index` feature {}",
        core::mem::size_of::<slot_list::DefaultIx>() * 8,
        core::mem::size_of::<slot_list::DefaultIx>(),
        if cfg!(feature = "u32-index") {
            "on"
        } else {
            "off"
        }
    );
    println!(
        "Global allocator: {} (`mimalloc` feature {})",
        if cfg!(feature = "mimalloc") {
            "mimalloc"
        } else {
            "system malloc"
        },
        if cfg!(feature = "mimalloc") {
            "on"
        } else {
            "off"
        }
    );

    // The function generated by `criterion_group!` builds its own `Criterion` (with `configure_from_args`)
    benches();
}
