//! 三种双向链表布局（同一套 Vec-index + free-list 算法）：
//!
//! - [`my_deque`]        —— SoA：`data` / `prev` / `next` 三个独立 `Vec`
//! - [`my_deque_packed`] —— 中间形态：`data` 一个 `Vec`，`prev`/`next` 打包进 `Link`
//! - [`my_deque_aos`]    —— AoS：`data` / `prev` / `next` 放进单个 `Node`
//!
//! 基准测试见 `benches/deque.rs`（criterion）。

pub mod my_deque;
pub mod my_deque_aos;
pub mod my_deque_packed;
