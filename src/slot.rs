//! 稳定句柄：指向连续内存里某个槽位的标识。

/// 槽位句柄：**元素存活期内稳定**的标识，可以存进别的容器（哈希表、邻接表、撤销栈…）。
///
/// 它指向的是**物理槽位**（`slot`），不是**逻辑位置**（`pos`）——两者没有换算公式，
/// 见 crate 文档的「两个位置」一节。拿句柄： [`Cursor::slot`](crate::Cursor::slot) /
/// [`CursorMut::slot`](crate::CursorMut::slot) / [`List::front_slot`](crate::List::front_slot)。
/// 用句柄： [`List::cursor_at`](crate::List::cursor_at) / [`List::remove_slot`](crate::List::remove_slot)
/// / [`List::move_to_front`](crate::List::move_to_front) —— 都是 `O(1)`。
///
/// 生命周期：`remove` / `pop_*` / `clear` 会把槽位挂回 free-list，此后旧 `Slot`
/// **不再有效**（`cursor_at` / `remove_slot` 返回 `None`）。但**不做世代校验**：
/// 若那个槽位被后来的插入复用了，旧 `Slot` 会指向**新元素**（经典 ABA）。要"陈旧
/// 句柄也能被测出来"，得用 `slotmap` / `fast-list` 那一类带世代号的实现。
///
/// 是否有效是**O(1) 可判定**的：槽位的空闲标记位就存在 `prev` 的最高位，判定时不碰
/// `data`（空闲槽的 `data` 未初始化，碰它是 UB）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Slot(pub(crate) usize);

impl Slot {
    /// 槽位内部下标（调试/序列化用；正常用法是把它当不透明句柄）。
    pub const fn to_usize(self) -> usize {
        self.0
    }
}
