//! 一个双向链表，节点全部放在**预分配的连续区域**里：下标即指针，删除后的
//! 槽位用 free-list 串起来复用（LIFO）。这就是名字 **`slot-list`** 的由来——
//! 每个节点就是连续区域里的一个 slot，`List::at` 直接按下标定位。
//!
//! 三种内存布局共用**同一份实现**：
//!
//! | 别名 | 布局 | 结构 |
//! |---|---|---|
//! | [`SoaList`] | [`Soa`] | `data` / `prev` / `next` 三个独立 `Vec` |
//! | [`PackedList`] | [`Packed`] | `data` 一个 `Vec`，`prev`/`next` 打包成 `Link` |
//! | [`AosList`] | [`Aos`] | 三者放进一个 `Node` |
//!
//! 三者都是 [`List<T, S>`] 的别名，所有逻辑只写一遍，布局差异由
//! [`Storage`] 策略提供。
//!
//! # 与 `std::collections::LinkedList` 的对齐
//!
//! 公开 API 与语义都对齐 `LinkedList`：
//!
//! - `new`（`const fn`）、`Default`、`len`、`is_empty`、`clear`
//! - `push_front` / `push_back`（返回 `()`）、`push_front_mut` / `push_back_mut`
//! - `pop_front` / `pop_back`
//! - `front` / `back` / `front_mut` / `back_mut`
//! - `cursor_front` / `cursor_front_mut` / `cursor_back` / `cursor_back_mut`
//! - `iter` / `iter_mut` / `IntoIterator`（`&`、`&mut`、所有权）
//! - `contains`、`retain`、`append`、`split_off`、`remove`
//! - `Clone`、`Debug`、`PartialEq`、`Eq`、`FromIterator`、`Extend`
//! - 游标：[`Cursor`] / [`CursorMut`]，含“幽灵”位置、环形移动、
//!   `insert_before` / `insert_after` / `remove_current` / `push_*` / `pop_*`
//!   / `peek_*` / `as_cursor` / `as_list`
//!
//! 语义差异：`append` 不是 std 的 O(1) 指针拼接，而是把对方整段槽位搬过来
//! （下标**一边搬一边加偏移**，索引只写一遍），O(other 的槽位总数)；
//! **对方的空闲槽也一起接过来**。想要"逐元素搬到末尾、优先复用自己已有的空闲
//! 槽"（按活元素计费，对方稀疏时特别划算），用 [`List::append_elementwise`]。
//! 两条路的取舍与实测见该方法与 [`Storage::append`] 的文档。
//!
//! # 实现要点：数组里没有哨兵值
//!
//! `prev` / `next` 数组里**每个值都是合法槽位下标**：两条链两端的"哑元"
//! 字段没人读，但同样是合法下标；`NIL` 只作为**标量**参数和游标的幽灵位置
//! tag 使用。这正是 [`List::append`] 能把搬过来的整段下标无条件 `+= base`
//! 的原因。
//!
//! 由此有两条纪律：
//!
//! - 沿链走**必须按个数**（两端的哑元不是 NIL）：[`Iter`] / [`IterMut`] 用
//!   `remaining` 计数，`retain` / `clear` 按 `len` 收尾；
//! - **四个端点字段在链为空时都归位 `NIL`**（`head`/`tail` 看 `len == 0`，
//!   `free_head`/`free_tail` 看 free 链变空）：这样"空不空"就是读一个字段，
//!   而不必算 `storage.len() - len`——后者每次 free/alloc 都要跑，实测
//!   `churn` 慢 3~6%；归位只在"变空"那一次写两个字段。
//!
//! # 本库扩展（std 没有）
//!
//! - [`List::at`]：O(min(pos, len-1-pos)) 的随机定位，std 完全没有随机访问
//! - [`List::with_capacity`] / [`List::reserve`]：预分配槽位容量
//!   （节点在连续内存里，所以 std 的链表没有这个需求，我们有）
//! - [`CursorMut::seek`] / [`CursorMut::move_steps`]：游标按位置/步长移动
//! - [`CursorMut::is_head`] / [`CursorMut::is_tail`]：O(1) 端点判定
//!
//! # 性能
//!
//! 数字、机制解释、`append` 两个 API 的选型矩阵，以及**已试过并否掉的优化**都在仓库
//! 根目录的 **`PERFORMANCE.md`**；基准本体在 `benches/list.rs`，它的表头写着读数纪律
//! （噪声底、缺页计数、交替 A/B）。
//!
//! # 有意不实现
//!
//! `PartialOrd` / `Ord` / `Hash`、`extract_if` / `retain_mut`、
//! `CursorMut::splice_before` / `splice_after` / `split_before` /
//! `split_after` / `remove_current_as_list`、分配器 API（`new_in`）。

mod cursor;
mod iter;
mod list;
pub mod storage;

#[cfg(test)]
mod tests;

pub use cursor::{Cursor, CursorMut};
pub use iter::{IntoIter, Iter, IterMut};
pub use list::List;
pub use storage::{Aos, Packed, Soa, Storage};

/// SoA 布局：`data` / `prev` / `next` 三个独立 `Vec`。
pub type SoaList<T> = List<T, Soa<T>>;

/// Packed 布局：`data` 一个 `Vec`，`prev`/`next` 交错放在另一个 `Vec`。
pub type PackedList<T> = List<T, Packed<T>>;

/// AoS 布局：`data` / `prev` / `next` 放进单个 `Node`。
pub type AosList<T> = List<T, Aos<T>>;
