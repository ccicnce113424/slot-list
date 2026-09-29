//! 一个双向链表，节点全部放在**预分配的连续区域**里：**槽位下标就是元素的身份**
//! （`slot`，在元素存活期内不变），删除后的槽位用 free-list 串起来复用（LIFO）。
//! 这就是名字 **`slot-list`** 的由来。
//!
//! # 两个"位置"：逻辑位置 `pos` 与物理槽位 `slot`
//!
//! 内部变量与文档里把这两个词彻底分开，**它们之间没有换算公式**：
//!
//! | | 逻辑位置 `pos` | 物理槽位 `slot` |
//! |---|---|---|
//! | 含义 | 元素在链上的次序（第几个） | 元素在连续内存里的下标（= 身份） |
//! | 范围 | `0 .. len()` | `0 .. storage.slots()` |
//! | 稳定性 | 每次插入/删除都会变 | **元素存活期内不变**（可当句柄用） |
//! | 怎么拿到 | 从端点走链；`Cursor::index()` | **句柄**：`Cursor::slot` / `CursorMut::slot` /
//!   `List::front_slot` / `back_slot` / `iter_slots`（全 `O(1)`）；类型是 [`Slot`]（裸下标
//!   8 B，**不带世代号** ⇒ 陈旧句柄查不出来；与 `fast-list` 的取舍对照见 `PERFORMANCE.md` §9） |
//! | 换算 | `slot_at(pos)`：走链 `O(min(pos, len-1-pos))` | `pos_of(slot)`：走链 `O(len)` |
//!
//! 公开 API 里带"位置"的都指**逻辑位置**：[`List::at`] / [`List::remove`] /
//! [`Cursor::index`]（与 std 的 `Cursor::index` 一致：返回逻辑位置，幽灵位置为 `None`）/
//! [`CursorMut::seek`] / [`CursorMut::move_steps`]。
//!
//! **槽位是对外的一等公民**：拿到句柄之后可以 `cursor_at(_mut)` / `remove_slot` /
//! `move_to_front` / `move_to_back` / `pos_of`，都不用先知道逻辑位置；不变量保证它在
//! **元素存活期内**有效、插入删除不搬动别的元素。元素被删除后同一槽位会被复用，
//! 于是**旧句柄不会失效**——它可能指向新元素（经典 ABA；同机对照与取舍见
//! `PERFORMANCE.md` §9）。`head` / `tail` / `free_head` / `free_tail` 与 `storage`
//! 里的下标仍然不公开。
//!
//! 三种内存布局共用**同一份实现**：
//!
//! | 别名 | 布局 | 每槽（T=8 / T=64） | 结构 |
//! |---|---|---|---|
//! | [`SoaList`] | [`Soa`] | 16 / 72 B | `data` / `prev` / `next` 三个独立 `Vec` |
//! | [`PackedList`] | [`Packed`] | 16 / 72 B | `data` 一个 `Vec`，`prev`/`next` 放进同一个 `Link` |
//! | [`AosList`] | [`Aos`] | 16 / 72 B | 三者放进一个 `Node` |
//!
//! 别名第二个参数是**索引宽度**（[`Ix`]），默认 `u32`：每槽比 `usize` 省 1/3，
//! `append` 快 37%，其余持平；代价是槽位上限 2.1G（见 [`Ix::MAX_SLOTS`]）。
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
//! `next` 数组里**每个值都是合法槽位下标**（两条链两端的"哑元"字段没人读，但同样是
//! 合法下标）；`NIL` 只作为**标量**参数（`head`/`tail`/`free_head`/`free_tail` 的"空"）
//! 和游标的幽灵位置 tag 使用。这正是 [`List::append`] 能把搬过来的整段下标无条件
//! `+= base` 的原因——**空闲标记位也搭这趟车**：它在 `prev` 的最高位，
//! `2^63 | v` 加上 `base` 仍是 `2^63 | (v + base)`（只要不溢出 `u64`）。
//!
//! `prev` 只在**live** 槽位上是"合法下标"；**空闲槽位的 `prev` 是陈旧值 + 标记位**
//! ⇒ 因此有一条契约：**只在 live 槽位上读 `prev`**（[`Storage::prev`] 不做掩码，
//! debug 构建里用 `debug_assert` 钉着；release 零成本）。
//!
//! 判定"槽位活没活"只看那个标记位，**绝不读 `data`**：空闲槽的 `data` 是未初始化的，
//! 碰它就是 UB。
//!
//! 由此有两条纪律：
//!
//! - 沿链走**必须按个数**（两端的哑元不是 NIL）：[`Iter`] / [`IterMut`] 用
//!   `remaining` 计数，`retain` / `clear` 按 `len` 收尾；
//! - **四个端点字段在链为空时都归位 `NIL`**（`head`/`tail` 看 `len == 0`，
//!   `free_head`/`free_tail` 看 free 链变空）：这样"空不空"就是读一个字段，
//!   而不必算 `storage.slots() - len`——后者每次 free/alloc 都要跑，实测
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
mod slot;
pub mod storage;

#[cfg(test)]
mod tests;

pub use cursor::{Cursor, CursorMut};
pub use iter::{IntoIter, Iter, IterMut};
pub use list::List;
pub use slot::Slot;
pub use storage::{Aos, Ix, Packed, Soa, Storage};

/// SoA 布局：`data` / `prev` / `next` 三个独立 `Vec`。
///
/// 第二个参数是**索引宽度**（[`Ix`]：`u8` / `u16` / `u32` / `u64` / `usize`），
/// 直接决定每槽多少字节：`T = 8` 时 `usize` 是 24 B/槽、`u32` 是 **16 B**、`u16` 是 12 B。
/// 代价是槽位数上限（`u32` ⇒ 2.1G、`u16` ⇒ 32k、`u8` ⇒ 128，超过就 panic）。
pub type SoaList<T, I = u32> = List<T, Soa<T, I>>;

/// Packed 布局：`data` 一个 `Vec`，`prev`/`next` 交错放在另一个 `Vec`。
/// 第二个参数是索引宽度（同 [`SoaList`]）。
pub type PackedList<T, I = u32> = List<T, Packed<T, I>>;

/// AoS 布局：`data` / `prev` / `next` 放进单个 `Node`。
/// 第二个参数是索引宽度（同 [`SoaList`]）。
pub type AosList<T, I = u32> = List<T, Aos<T, I>>;
