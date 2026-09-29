//! 三种内存布局（`SoaList` / `PackedList` / `AosList`）与
//! std `LinkedList`、`VecDeque`、`Vec`、以及 `fast-list`（slotmap + 世代号句柄）的横向基准。
//!
//! 三种布局是同一个泛型类型 `List<T, S>` 的不同 `Storage` 参数，所以基准体用宏生成
//! 三种实例，不手写三份；基线各自手写。
//!
//! 运行：`cargo bench [-- <过滤正则>]`
//!
//! **stable 上也能编**：只有对比 std `LinkedList` **游标**的那三行
//! （`middle_insert_remove` / `random_remove_insert` / `cursor_update` 的 `LinkedList`）
//! 需要 `#![feature(linked_list_cursors)]`，它们由 `linked-list-cursors` feature 控制
//! ——**该 feature 不在 `default` 里**，所以 stable 上 `cargo bench` 直接可用，
//! nightly 上想要那三行就显式开：
//!
//! ```text
//! cargo bench                                                    # 任何通道都能跑
//! cargo bench --features linked-list-cursors                     # 只有 nightly 能开（要那三行对照）
//! ```
//!
//! 用 feature 而不是 `build.rs` 自动探测通道，是因为这是个**库**：`build.rs` 会在每个
//! 下游用户编译本 crate 时都跑一次，而 feature 是显式的、下游零成本，也和 `mimalloc`
//! 的做法一致。
//! 报告：`target/criterion/report/index.html`
//!
//! # 读数纪律（先看这段）
//!
//! - **噪声底 ±5%**；`append*` / `blob_*` 这些大分配组 **±10 ~ 20%**。同一份代码在
//!   不同时刻能漂这么多（实测 `churn` 2.80 ~ 3.01 ms），而外部基线可能纹丝不动 ⇒
//!   判定回归要两版**交替**跑（A/B/A/B），并同时看基线漂了多少。
//! - `end_ops` / `churn` / `append*` 量的是**分配器与内核**，不是我们的代码：`append`
//!   同一份代码能从 2 ms 漂到 16 ms（计时区缺页 0 → 8187）。比较时看**缺页数**。
//! - 大页只能靠环境变量（本机 THP 是 `madvise` 模式且 `nr_hugepages=0`，库内
//!   `madvise` 时好时坏）：`GLIBC_TUNABLES=glibc.malloc.hugetlb=1`，或 mimalloc 的
//!   页复用 / `MIMALLOC_PURGE_DELAY=-1`。
//!
//! # 各组怎么读、数字与机制
//!
//! 全部在仓库根目录的 **`PERFORMANCE.md`**：三种布局的内存/地址表（实测核对）、
//! 全套结果、`append` 两个 API 的字节对账与选型矩阵、内部探针（`clear` / `Drop` / 迭代三层 /
//! 带宽对账），以及**已试过并否掉的优化**（分块布局、顺行位图、迭代器探路、u32 索引、
//! `reserve`、`madvise` …），每条都带实测数字与代价。

#![cfg_attr(feature = "linked-list-cursors", feature(linked_list_cursors))]

// 全局分配器。`cargo bench --no-default-features` 会切回系统 malloc，
// 便于对比"分配器是否把内存还给内核"对端操作的影响。
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use slot_list::{AosList, PackedList, Slot, SoaList};

use criterion::{BatchSize, Bencher, Criterion, criterion_group, criterion_main};

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
// 我们三种布局的基准体
//
// 三种布局共用一份实现（`List<T, S>`），所以基准体也用宏生成三种实例，
// 而不是手写三份拷贝。布局只体现在 `$ty` 上。
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
                // 在当前位置之前插入，游标不动。
                cursor.insert_before(black_box(i));

                // 移到刚插入的节点。
                cursor.move_prev();

                // 删掉它；游标回到原来的节点。
                let value = cursor.remove_current().unwrap();

                black_box(value);
            }

            black_box(cursor.index());
        }
    };
}

/// 随机位置"删一个 + 原地插回"：
/// - `by_handle`：`cursor_at(handle)` —— **O(1)** 入口；
/// - `by_pos`   ：`at(pos)`          —— O(N) 走链入口。
///
/// 两者**做的工作完全一样**，唯一区别是进入方式。
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

                // 游标已指向下一个元素（删的是尾元素时为幽灵位置，
                // insert_before 会把新元素追加到末尾）。
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

/// 整块搬运：`a.append(&mut b)`（输入由 `iter_batched` 在计时区外造好）。
///
/// **返回值必须把两个链表交回去**：否则它们会在计时区内析构，而"析构一个
/// 2M 节点的链表"会把这个基准彻底带偏（各基线的析构代价差得也很远：
/// `LinkedList` 是 2M 次 `Box` 释放，`Vec` 是零）。交给 criterion 之后，
/// 析构发生在计时之后。
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

// ---- 实例化（三种布局 × 12 个基准）----

bench_end_ops!(
    soalist_push_back_pop_front,
    SoaList<usize>,
    push_back,
    pop_front
);
bench_end_ops!(
    packedlist_push_back_pop_front,
    PackedList<usize>,
    push_back,
    pop_front
);
bench_end_ops!(
    aoslist_push_back_pop_front,
    AosList<usize>,
    push_back,
    pop_front
);

bench_end_ops!(
    soalist_push_front_pop_back,
    SoaList<usize>,
    push_front,
    pop_back
);
bench_end_ops!(
    packedlist_push_front_pop_back,
    PackedList<usize>,
    push_front,
    pop_back
);
bench_end_ops!(
    aoslist_push_front_pop_back,
    AosList<usize>,
    push_front,
    pop_back
);

bench_construct!(make_soalist, SoaList<usize>);
bench_construct!(make_packedlist, PackedList<usize>);
bench_construct!(make_aoslist, AosList<usize>);

bench_iter!(soalist_iter, SoaList<usize>);
bench_iter!(packedlist_iter, PackedList<usize>);
bench_iter!(aoslist_iter, AosList<usize>);

bench_at_middle!(soalist_at_middle, SoaList<usize>);
bench_at_middle!(packedlist_at_middle, PackedList<usize>);
bench_at_middle!(aoslist_at_middle, AosList<usize>);

bench_insert_before_remove!(soalist_insert_before_remove, SoaList<usize>);
bench_insert_before_remove!(packedlist_insert_before_remove, PackedList<usize>);
bench_insert_before_remove!(aoslist_insert_before_remove, AosList<usize>);

bench_churn!(soalist_churn, SoaList<usize>);
bench_churn!(packedlist_churn, PackedList<usize>);
bench_churn!(aoslist_churn, AosList<usize>);

bench_random_remove_insert!(soalist_random_remove_insert, SoaList<usize>);
bench_random_remove_insert!(packedlist_random_remove_insert, PackedList<usize>);
bench_random_remove_insert!(aoslist_random_remove_insert, AosList<usize>);

bench_cursor_update!(soalist_cursor_update, SoaList<usize>);
bench_cursor_update!(packedlist_cursor_update, PackedList<usize>);
bench_cursor_update!(aoslist_cursor_update, AosList<usize>);

bench_append!(soalist_append, SoaList<usize>);
bench_append!(packedlist_append, PackedList<usize>);
bench_append!(aoslist_append, AosList<usize>);

// 大载荷（64 B）版本：数据拷贝占主导，布局差异才会显出来。
bench_append!(soalist_blob_append, SoaList<Blob64>);
bench_append!(packedlist_blob_append, PackedList<Blob64>);
bench_append!(aoslist_blob_append, AosList<Blob64>);

bench_construct_blob!(make_soalist_blob, SoaList<Blob64>);
bench_construct_blob!(make_packedlist_blob, PackedList<Blob64>);
bench_construct_blob!(make_aoslist_blob, AosList<Blob64>);

bench_blob_iter!(soalist_blob_iter, SoaList<Blob64>);
bench_blob_iter!(packedlist_blob_iter, PackedList<Blob64>);
bench_blob_iter!(aoslist_blob_iter, AosList<Blob64>);

bench_blob_end_ops!(soalist_blob_push_back_pop_front, SoaList<Blob64>);
bench_blob_end_ops!(packedlist_blob_push_back_pop_front, PackedList<Blob64>);
bench_blob_end_ops!(aoslist_blob_push_back_pop_front, AosList<Blob64>);

// ============================================================
// 基线：std VecDeque / std LinkedList / Vec
// ============================================================

// ---- 输入生成与游标定位 ----

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

// ---- 端操作 ----

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

// ---- 构造 ----

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

// ---- 纯迭代 ----

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

// ---- 中间位置访问 ----

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

// ---- 已知位置的局部插入/删除 ----

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

// ---- 稳态 churn ----

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

// ---- 随机位置删除 + 插入 ----

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

// ---- 已知位置读写 ----

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

// ---- append（整块搬运）----

fn vecdeque_append(
    mut a: VecDeque<usize>,
    mut b: VecDeque<usize>,
) -> (VecDeque<usize>, VecDeque<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

/// std 的 `LinkedList::append` 是 O(1) 指针拼接（不搬元素）。
fn linkedlist_append(
    mut a: LinkedList<usize>,
    mut b: LinkedList<usize>,
) -> (LinkedList<usize>, LinkedList<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

/// `Vec` 是"整块 memcpy"的天然上限，用来标定"复制 + 修下标"有多贵。
fn vec_append(mut a: Vec<usize>, mut b: Vec<usize>) -> (Vec<usize>, Vec<usize>) {
    a.append(&mut b);

    black_box(a.len());

    (a, b)
}

// ============================================================
// 64 字节元素
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

// ---- 64B 构造 / 迭代 / 端操作 ----

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

// ---- 64B 的"有空闲槽"构造器：给 append_elementwise 用（LARGE_N 活 + LARGE_N 空闲） ----

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

make_roomy_blob!(make_soalist_blob_roomy, SoaList<Blob64>);
make_roomy_blob!(make_packedlist_blob_roomy, PackedList<Blob64>);
make_roomy_blob!(make_aoslist_blob_roomy, AosList<Blob64>);

// ---- 64B append（与 usize 版同形态：两边都由 push/collect 构造 ⇒ 目标要扩容） ----

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

bench_slot_vs_pos!(soalist_by_handle, soalist_by_pos, SoaList<usize>);
bench_slot_vs_pos!(packedlist_by_handle, packedlist_by_pos, PackedList<usize>);
bench_slot_vs_pos!(aoslist_by_handle, aoslist_by_pos, AosList<usize>);

// ============================================================
// 对照组：`fast-list`（slotmap 索引 + 世代号）
//
// API 形状不同，基准体单独写、不套宏：
// - 句柄是 `LinkedListIndex`（slotmap 的 key，带世代号；`contains_key` 即 ABA 校验）；
// - `get`/`remove`/`insert_before` 都收句柄；`nth(pos)` 是 O(N) 走链；
// - **没有游标**：删掉一个元素后它给不出"我原来在哪"，这里只能自己从
//   `LinkedListItem::next_index`/`prev_index` 里挑锚点接回去（我们那边游标留在原位）；
// - `iter()` 产出 `&LinkedListItem<T>`，值在 `.value`；
// - 没有 `append` / `move_to_front` / `move_to_back`（功能对照见 PERFORMANCE.md）。
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

/// "删一个 + 原地插回"：删掉之后用它自己给的后继（没有就用前驱）当锚点接回去。
///
/// 注意这里**不能** `unwrap`：`fast-list` 的句柄带世代号，一次 `remove` + 重新插入
/// 之后原句柄就作废了（新元素拿新世代）。基准里由调用方用 `contains_key` 先判。
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

/// 句柄入口。**每次都要 `contains_key` 校验**：上一轮的"删了又插回"已经让这批
/// 句柄作废了，失效就得按位置重找一遍。这一步是世代号语义的真实成本，不是我们加的。
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
// 索引宽度对照：`SoaList<usize, u32>` vs `SoaList<usize, usize>`
//
// 每槽开销 = `T` + 两条链接 ⇒ `T = 8` 时 16 B vs 24 B。谁受益取决于
// 链接数组的访存占比：走链（`at(pos)`）与迭代最明显，端操作/句柄入口最小。
// ============================================================

bench_construct!(make_soa32, SoaList<usize, u32>);
bench_construct!(make_soa64, SoaList<usize, usize>);
bench_churn!(soa32_churn, SoaList<usize, u32>);
bench_churn!(soa64_churn, SoaList<usize, usize>);
bench_iter!(soa32_iter, SoaList<usize, u32>);
bench_iter!(soa64_iter, SoaList<usize, usize>);
bench_at_middle!(soa32_at_middle, SoaList<usize, u32>);
bench_at_middle!(soa64_at_middle, SoaList<usize, usize>);
bench_append!(soa32_append, SoaList<usize, u32>);
bench_append!(soa64_append, SoaList<usize, usize>);

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
    g.bench_function("SoaList", |b| b.iter(soalist_push_back_pop_front));
    g.bench_function("PackedList", |b| b.iter(packedlist_push_back_pop_front));
    g.bench_function("AosList", |b| b.iter(aoslist_push_back_pop_front));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_push_back_pop_front));
    g.bench_function("LinkedList", |b| b.iter(linkedlist_push_back_pop_front));
    g.bench_function("FastList", |b| b.iter(fastlist_push_back_pop_front));
    g.finish();

    let mut g = c.benchmark_group("end_ops/push_front_pop_back");
    g.bench_function("SoaList", |b| b.iter(soalist_push_front_pop_back));
    g.bench_function("PackedList", |b| b.iter(packedlist_push_front_pop_back));
    g.bench_function("AosList", |b| b.iter(aoslist_push_front_pop_back));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_push_front_pop_back));
    g.bench_function("LinkedList", |b| b.iter(linkedlist_push_front_pop_back));
    g.finish();
}

fn iteration(c: &mut Criterion) {
    let soalist = make_soalist();
    let packedlist = make_packedlist();
    let aoslist = make_aoslist();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let vec = make_vec();
    let fastlist = make_fastlist();

    let mut g = c.benchmark_group("iteration");
    g.bench_function("SoaList", |b| b.iter(|| soalist_iter(&soalist)));
    g.bench_function("PackedList", |b| b.iter(|| packedlist_iter(&packedlist)));
    g.bench_function("AosList", |b| b.iter(|| aoslist_iter(&aoslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_iter(&vecdeque)));
    g.bench_function("LinkedList", |b| b.iter(|| linkedlist_iter(&linkedlist)));
    g.bench_function("Vec", |b| b.iter(|| vec_iter(&vec)));
    g.bench_function("FastList", |b| b.iter(|| fastlist_iter(&fastlist)));
    g.finish();
}

fn middle_access(c: &mut Criterion) {
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let fastlist = make_fastlist();

    let mut g = c.benchmark_group("middle_access");
    g.bench_function("SoaList", |b| b.iter(|| soalist_at_middle(&mut soalist)));
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_at_middle(&mut packedlist))
    });
    g.bench_function("AosList", |b| b.iter(|| aoslist_at_middle(&mut aoslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_index_middle(&vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_at_middle(&linkedlist))
    });
    g.bench_function("FastList", |b| b.iter(|| fastlist_at_middle(&fastlist)));
    g.finish();
}

fn middle_insert_remove(c: &mut Criterion) {
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

    let mut g = c.benchmark_group("middle_insert_remove");
    g.bench_function("SoaList", |b| {
        b.iter(|| soalist_insert_before_remove(&mut soalist))
    });
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_insert_before_remove(&mut packedlist))
    });
    g.bench_function("AosList", |b| {
        b.iter(|| aoslist_insert_before_remove(&mut aoslist))
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
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

    let mut g = c.benchmark_group("churn");
    g.bench_function("SoaList", |b| b.iter(|| soalist_churn(&mut soalist)));
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_churn(&mut packedlist))
    });
    g.bench_function("AosList", |b| b.iter(|| aoslist_churn(&mut aoslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_churn(&mut vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_churn(&mut linkedlist))
    });
    g.bench_function("FastList", |b| b.iter(|| fastlist_churn(&mut fastlist)));
    g.finish();
}

fn random_remove_insert(c: &mut Criterion) {
    let positions = make_random_positions(N, RANDOM_OPS);
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();
    let fast_handles: Vec<LinkedListIndex> = positions
        .iter()
        .map(|&pos| fastlist.nth(pos).unwrap())
        .collect();

    let mut g = c.benchmark_group("random_remove_insert");
    g.bench_function("SoaList", |b| {
        b.iter(|| soalist_random_remove_insert(&mut soalist, &positions))
    });
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_random_remove_insert(&mut packedlist, &positions))
    });
    g.bench_function("AosList", |b| {
        b.iter(|| aoslist_random_remove_insert(&mut aoslist, &positions))
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
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    #[cfg(feature = "linked-list-cursors")]
    let mut linkedlist = make_linkedlist();

    let mut g = c.benchmark_group("cursor_update");
    g.bench_function("SoaList", |b| {
        b.iter(|| soalist_cursor_update(&mut soalist))
    });
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_cursor_update(&mut packedlist))
    });
    g.bench_function("AosList", |b| {
        b.iter(|| aoslist_cursor_update(&mut aoslist))
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

    g.bench_function("SoaList", |b| {
        b.iter_batched(
            || (make_soalist(), make_soalist()),
            |(a, b)| soalist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("PackedList", |b| {
        b.iter_batched(
            || (make_packedlist(), make_packedlist()),
            |(a, b)| packedlist_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("AosList", |b| {
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

    g.bench_function("SoaList", |b| {
        b.iter_batched(
            || (make_soalist_blob(), make_soalist_blob()),
            |(a, b)| soalist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("PackedList", |b| {
        b.iter_batched(
            || (make_packedlist_blob(), make_packedlist_blob()),
            |(a, b)| packedlist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    g.bench_function("AosList", |b| {
        b.iter_batched(
            || (make_aoslist_blob(), make_aoslist_blob()),
            |(a, b)| aoslist_blob_append(a, b),
            BatchSize::PerIteration,
        )
    });
    // 逐元素路径：目标预先有 LARGE_N 个空闲槽（它的推荐用法）
    g.bench_function("SoaList::elementwise", |b| {
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
    g.bench_function("PackedList::elementwise", |b| {
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
    g.bench_function("AosList::elementwise", |b| {
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
    let soalist = make_soalist_blob();
    let packedlist = make_packedlist_blob();
    let aoslist = make_aoslist_blob();
    let vecdeque = make_vecdeque_blob();
    let linkedlist = make_linkedlist_blob();
    let vec = make_vec_blob();

    let mut g = c.benchmark_group("blob_iter");
    g.bench_function("SoaList", |b| b.iter(|| soalist_blob_iter(&soalist)));
    g.bench_function("PackedList", |b| {
        b.iter(|| packedlist_blob_iter(&packedlist))
    });
    g.bench_function("AosList", |b| b.iter(|| aoslist_blob_iter(&aoslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_blob_iter(&vecdeque)));
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_blob_iter(&linkedlist))
    });
    g.bench_function("Vec", |b| b.iter(|| vec_blob_iter(&vec)));
    g.finish();
}

/// 句柄入口（O(1)）vs 逻辑位置入口（O(N)）：**同一个工作**，只有入口不同。
/// 这一组就是"要不要把 Slot 暴露出来"的量化依据。
fn slot_entry(c: &mut Criterion) {
    let positions = make_random_positions(N, RANDOM_OPS);
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();

    // 句柄在计时区外先取好（取的时候要站到那个位置，是 O(N)）
    let soa_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| soalist.at(pos).unwrap().slot().unwrap())
        .collect();
    let packed_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| packedlist.at(pos).unwrap().slot().unwrap())
        .collect();
    let aos_handles: Vec<Slot> = positions
        .iter()
        .map(|&pos| aoslist.at(pos).unwrap().slot().unwrap())
        .collect();
    let mut fastlist = make_fastlist();
    let fast_handles: Vec<LinkedListIndex> = positions
        .iter()
        .map(|&pos| fastlist.nth(pos).unwrap())
        .collect();

    let mut g = c.benchmark_group("slot_entry");
    g.bench_function("SoaList::by_handle", |b| {
        b.iter(|| soalist_by_handle(&mut soalist, &soa_handles))
    });
    g.bench_function("SoaList::by_pos", |b| {
        b.iter(|| soalist_by_pos(&mut soalist, &positions))
    });
    g.bench_function("PackedList::by_handle", |b| {
        b.iter(|| packedlist_by_handle(&mut packedlist, &packed_handles))
    });
    g.bench_function("PackedList::by_pos", |b| {
        b.iter(|| packedlist_by_pos(&mut packedlist, &positions))
    });
    g.bench_function("AosList::by_handle", |b| {
        b.iter(|| aoslist_by_handle(&mut aoslist, &aos_handles))
    });
    g.bench_function("AosList::by_pos", |b| {
        b.iter(|| aoslist_by_pos(&mut aoslist, &positions))
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
    g.bench_function("SoaList", |b| b.iter(soalist_blob_push_back_pop_front));
    g.bench_function("PackedList", |b| {
        b.iter(packedlist_blob_push_back_pop_front)
    });
    g.bench_function("AosList", |b| b.iter(aoslist_blob_push_back_pop_front));
    g.bench_function("VecDeque", |b| b.iter(vecdeque_blob_push_back_pop_front));
    g.bench_function("LinkedList", |b| {
        b.iter(linkedlist_blob_push_back_pop_front)
    });
    g.finish();
}

// ============================================================
// clear / Drop 探针
//
// 量两件事（都是用 `iter_batched` 把"造数据/析构"挪出计时区）：
//
// 结论（1M 槽位、Soa）：`clear` 保留追链表。扫描版只在"没有 Drop glue 且几乎满"时
// 快 2.2×，稀疏时慢 327×（1.42 µs → 464 µs）、带 Drop glue 时连密集也慢 12%；
// `Drop` 只走 live 链就地析构则全面胜出——无 glue 1.172 ms → 2.37 µs，带 glue
// Soa 3.13 → 1.96 ms（1.6×）。明细见 PERFORMANCE.md §4/§5。
//
// 1. `clear`：追链表 vs "按内存顺序扫一遍 data、用 `is_free` 判断"。
//    两种写法的代价模型完全不同——追链表是 O(live) 次**随机**槽位访问，
//    扫描是 O(slots) 次**顺序**访问。dense 看上限、sparse 看下限。
// 2. `Drop`：析构整条链表本来只需要"走 live 链、就地析构"，但现在它经由
//    `clear`，会顺带把每个槽位挂回 free 链——对一个马上要丢掉的结构是白做。
// ============================================================
macro_rules! bench_clear_drop {
    ($ty:ty, $maker:ident, $dense:ident, $sparse:ident, $refill:ident, $dropper:ident) => {
        /// 密集：`slots == len == N`。
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

        /// 稀疏：1M 个槽位里只剩 1000 个 live（free 链上挂着 999_000 个）。
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

        /// `clear` 之后重新填满：free 链的**顺序**决定复用时的局部性。
        /// 全程只在已分配的槽位里搬，不进分配器。
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

        /// 析构整条链表：理想代价 = O(live) 次就地析构，不做 free 链维护。
        fn $dropper(b: &mut Bencher) {
            b.iter_batched(|| $maker(), drop, BatchSize::PerIteration);
        }
    };
}

bench_clear_drop!(
    SoaList,
    make_soalist,
    soalist_clear_dense,
    soalist_clear_sparse,
    soalist_refill,
    soalist_drop
);
bench_clear_drop!(
    PackedList,
    make_packedlist,
    packedlist_clear_dense,
    packedlist_clear_sparse,
    packedlist_refill,
    packedlist_drop
);
bench_clear_drop!(
    AosList,
    make_aoslist,
    aoslist_clear_dense,
    aoslist_clear_sparse,
    aoslist_refill,
    aoslist_drop
);

/// 带 Drop glue 的 64 B 载荷。
///
/// 用 `usize` 量不出 `Drop`/`clear` 的真实差别：没有 glue 时整条析构链会被
/// LLVM 直接消掉（实测"析构 1M 个 `usize`"只要 1.86 µs）。这里让 `drop` 里
/// 真的读一下字段，把析构钉住。
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

make_drop64_list!(make_soalist_drop64, SoaList<Drop64>);
make_drop64_list!(make_packedlist_drop64, PackedList<Drop64>);
make_drop64_list!(make_aoslist_drop64, AosList<Drop64>);

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
    SoaList<Drop64>,
    make_soalist_drop64,
    soalist_glue_clear_dense,
    soalist_glue_drop
);
bench_clear_drop_glue!(
    PackedList<Drop64>,
    make_packedlist_drop64,
    packedlist_glue_clear_dense,
    packedlist_glue_drop
);
bench_clear_drop_glue!(
    AosList<Drop64>,
    make_aoslist_drop64,
    aoslist_glue_clear_dense,
    aoslist_glue_drop
);

fn clear_drop(c: &mut Criterion) {
    let mut g = c.benchmark_group("clear_drop");

    g.bench_function("dense/SoaList", soalist_clear_dense);
    g.bench_function("dense/PackedList", packedlist_clear_dense);
    g.bench_function("dense/AosList", aoslist_clear_dense);
    g.bench_function("sparse/SoaList", soalist_clear_sparse);
    g.bench_function("sparse/PackedList", packedlist_clear_sparse);
    g.bench_function("sparse/AosList", aoslist_clear_sparse);
    g.bench_function("refill/SoaList", soalist_refill);
    g.bench_function("refill/PackedList", packedlist_refill);
    g.bench_function("refill/AosList", aoslist_refill);
    g.bench_function("drop/SoaList", soalist_drop);
    g.bench_function("drop/PackedList", packedlist_drop);
    g.bench_function("drop/AosList", aoslist_drop);
    g.bench_function("gluedense/SoaList", soalist_glue_clear_dense);
    g.bench_function("gluedense/PackedList", packedlist_glue_clear_dense);
    g.bench_function("gluedense/AosList", aoslist_glue_clear_dense);
    g.bench_function("gluedrop/SoaList", soalist_glue_drop);
    g.bench_function("gluedrop/PackedList", packedlist_glue_drop);
    g.bench_function("gluedrop/AosList", aoslist_glue_drop);

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

criterion_main!(benches);
