//! 内存布局策略：`data` / `prev` / `next` 如何落到内存里。
//!
//! 三种策略的**算法完全相同**，只有存储方式不同：
//!
//! - [`Soa`]：三个独立 `Vec`（array of structs 的反面，structure of arrays）
//! - [`Packed`]：`data` 一个 `Vec`，`prev`/`next` 打包成 [`Link`] 交错存放
//! - [`Aos`]：三者放进一个 [`Node`]（array of structs）

use std::mem::{MaybeUninit, offset_of, size_of};

/// 「这一侧没有邻居」。
///
/// **只作为标量使用**：函数参数（`List::insert_link` 的 `prev` / `next`）和
/// 游标的幽灵位置 tag。它**绝不会出现在 `prev` / `next`
/// 数组里**——数组里每个值都是合法槽位下标（链表两端的"哑元"字段是"没人读
/// 的合法下标"，不是 NIL）。这是 [`Storage::append`] 能把整段下标直接加偏移
/// 的前提。
pub(crate) const NIL: usize = usize::MAX;

/// 「这个槽位当前空闲」的标记位，存在 `prev` 的**最高位**。
///
/// 为什么要它：句柄（[`Slot`](crate::Slot)）是指向槽位的裸标识，槽位会被 free-list
/// 复用 ⇒ 必须能 O(1) 判断"这个槽位现在活没活"，**否则就要对空闲槽的
/// `MaybeUninit` 做 `assume_init_*`（UB）**。
///
/// 为什么放在 `prev` 的最高位：
///
/// - **写入次数最少**：live 槽位的 `prev` 由链接写入覆盖（那些值天然没有这一位），
///   空闲槽位只被 free 链用到 `next` ⇒ 这一位**只在"变空闲"那一次**需要显式写，
///   链接写入路径上没有任何 read-modify-write；
/// - **`append` 的整段 `+= base` 自己会带着它走**：`2^63 | v` 加 `base` 仍是
///   `2^63 | (v + base)`，只要不溢出 `u64` 就不需要特例（`v + base < 2^63`，而
///   槽位数远小于此 ⇒ 实际永远成立，`debug_assert` 兜底）；
/// - **读值处一律掩码**：`Storage::prev` 返回掩码后的下标，`is_free` 只看那一位。
///
/// 代价：每个"变成空闲"的槽位一次 read-modify-write（`pop`/`remove`/`clear` 等
/// 每条回收路径一次）。
pub(crate) const FREE_BIT: usize = 1 << (usize::BITS - 1);

/// [`Iter`](crate::Iter) / [`IterMut`](crate::IterMut) 需要的裸地址布局。
///
/// 三种 `Storage` 把同样的三个逻辑字段放在完全不同的位置，迭代器又必须用裸地址
/// 走链（这样 `next()` 才能交出 `&'a mut T` 而不与自身状态打架），所以这里用
/// 「**基址 + 步长 + 偏移**」把三者统一描述成同一个公式（下标是槽位下标）：
///
/// ```text
/// 元素 i 的 data 地址 = data.cast::<u8>() + i * data_stride + data_offset
/// 元素 i 的 prev 值   = *(prev.cast::<u8>() + i * prev_stride)
/// 元素 i 的 next 值   = *(next.cast::<u8>() + i * next_stride)
/// ```
///
/// 字段（基址一律是**字节指针** `u8`，所以每次取址只需**一次** cast：`u8` → 目标
/// 类型。若把基址标成 `*mut MaybeUninit<T>`，反而要先 `cast::<u8>()` 做字节算术、
/// 再 cast 回来——两次）：
/// - `data`：元素 0 的 `data` 槽**基址**（槽位可能未初始化，读之前要
///   `assume_init_*`）；
/// - `data_stride`：相邻元素的 `data` 相隔多少字节（= 一个"元素"有多大）；
/// - `data_offset`：从 `data` 基址挪到元素 0 的 `data` 字段还要加多少字节
///   （只有把字段塞进节点里的布局才非 0，而且用 `offset_of!` 说出来，
///   不依赖"`data` 恰好在开头"这个假设）；
/// - `prev` / `next`：元素 0 的链接字段基址（三种布局的链接**都是 `usize`**，
///   所以类型固定，变的只有步长）；
/// - `prev_stride` / `next_stride`：相邻元素的链接字段相隔多少字节——Packed 里
///   两个链接打包成 `Link`，Aos 里它们和 `data` 同处一个 `Node<T>`，所以这里的
///   步长是"元素"大小而不是 8。
///
/// 三个布局具体怎么落到这些字段上（`T = usize`，括号里是 `T = [u64; 8]`）：
///
/// | 布局 | `data` 基址 / `data_stride` / `data_offset` | `prev`·`next` 基址 / 步长 |
/// |---|---|---|
/// | `Soa<T>` | 数据数组首址 / `size_of::<T>()` 8（64）/ 0 | 各自数组首址 / 8 |
/// | `Packed<T>` | 数据数组首址 / 8（64）/ 0 | `links + offset_of!(Link, …)` 0·8 / 16 |
/// | `Aos<T>` | 节点数组首址 / `size_of::<Node<T>>()` 24（80）/ `offset_of!(Node<T>, data)` 0 | `nodes + offset_of!(Node<T>, …)` 8·16（64·72）/ 24（80） |
///
/// ```text
/// Soa     data  [d0][d1][d2]…      prev [p0][p1]…      next [n0][n1]…
///                ↑ stride 8               ↑ stride 8           ↑ stride 8
/// Packed  data  [d0][d1]…          links [(p0,n0)][(p1,n1)]…
///                                          ↑ prev=links+0, next=links+8, stride 16
/// Aos     nodes [(d0,p0,n0)][(d1,p1,n1)]…
///                 ↑ data=nodes+0, prev=nodes+8, next=nodes+16, stride 24
/// ```
///
/// 地址能这么算，靠的是三条不变量：① 指针来自同一个容器自己的 `Vec`，而 `Layout`
/// 只在迭代器持有 `&'a mut List` 的那段独占期里用（中途不会 realloc / 搬家）；
/// ② 下标只落在已分配的槽位内；③ 数组里每个 `prev`/`next` 都是**合法下标**
/// （两端是"指向自己的哑元"，见 crate 文档），所以可以无条件读、也可以无条件
/// `+= base`——这正是 [`Storage::append`] 整块搬运的前提。
#[doc(hidden)]
pub struct Layout {
    pub(crate) data: *mut u8,
    pub(crate) data_stride: usize,
    pub(crate) data_offset: usize,
    pub(crate) prev: *const u8,
    pub(crate) prev_stride: usize,
    pub(crate) next: *const u8,
    pub(crate) next_stride: usize,
}

#[doc(hidden)]
pub mod sealed {
    /// 封闭 [`super::Storage`]，外部无法实现。
    pub trait Sealed {}
}

/// 存储策略。
///
/// 这个 trait 是**封闭**的：只由本 crate 的三种布局实现。它是
/// [`List`](crate::List) 的实现细节，出现在类型参数位置上。
pub trait Storage<T>: sealed::Sealed {
    /// 已分配的槽位总数（live + free）。
    fn slots(&self) -> usize;

    /// 是否没有任何槽位。
    fn is_empty(&self) -> bool {
        self.slots() == 0
    }

    /// 预留至少 `additional` 个槽位的容量（`len + additional`）。
    fn reserve(&mut self, additional: usize);

    /// 追加一个新槽位并返回其下标。
    ///
    /// 两条链接先初始化成指向自己的自环：调用方随后就会重写需要的那几条，
    /// 但**两端那两条"哑元"字段可能一直留着这个值**，所以它必须是合法下标
    /// （见 [`append`](Storage::append)：整段下标都要无条件加偏移）。
    fn grow(&mut self) -> usize;

    /// 把 `other` 的全部槽位接到 `self` 后面，并把搬过来的**每个下标都加上
    /// 偏移**（偏移量 = 搬运前 `self.slots()`，由实现自己取）。搬完 `other` 变空
    /// （容量留在它自己那里）。
    ///
    /// 关键是"一边复制一边把**已经更新过**的索引写进自己的 `Vec`"：索引只写
    /// 一遍（实测比"先整块 memcpy、再原地读改写修一遍"快 ~0.7ms/24MB，甚至
    /// 比纯 memcpy 三个数组还快——因为省掉了 16MB 读 + 16MB 写）。
    ///
    /// # 代价结构（实测，1M ⊕ 1M，24 MB 载荷）
    ///
    /// | 组成 | 时间 |
    /// |---|---|
    /// | 拷贝 + 下标改写（目标内存**已触碰**） | 1.6 ~ 2.3 ms（~25 GB/s） |
    /// | 再叠加 `realloc`（把旧数据 24 MB 搬进新块） | +1.9 ~ 3.1 ms |
    /// | 分配器若把大块还给内核：**每页 ~1.4 µs** 的物化费 | 4095 ~ 8187 页 ⇒ +5.7 ~ 11.5 ms |
    ///
    /// 前两项是算法成本（mimalloc 下实测真实 append 3.8 ~ 4.3 ms、0 缺页）；第三
    /// 项是"不及预期"的来源，而它只取决于分配器：
    ///
    /// | 分配器 | 真实 append | 缺页 |
    /// |---|---|---|
    /// | mimalloc（本仓库基准默认） | 3.8 ~ 4.3 ms | 0 |
    /// | 系统 malloc | 7.9 ~ 16 ms | 4095 ~ 8187 |
    ///
    /// 对照实验（决定性）：`mmap` 24 MB、**每页只写 1 字节、不做任何拷贝**，就要
    /// 8.2 ms / 5860 页——两种分配器下完全一致。也就是说这条路按**页**计费，缺页
    /// 才是大头，拷贝不是。分配器是否把大块还给内核（glibc 超过 128 KB 走
    /// mmap/munmap；mimalloc 靠段复用与延迟 purge）决定付不付这笔钱，也解释了同
    /// 一个 append 在不同时刻能从 2 ms 漂到 16 ms——比较时请同时看缺页数。
    ///
    /// 只有"目标内存已经触碰过"能真正省掉它——所以想要零新页就把元素填进自己已有的
    /// 空闲槽，即 [`crate::List::append_elementwise`]（复用已触碰的槽，实测 ~2.1 ms）。
    /// `madvise(MADV_HUGEPAGE)` 理论上
    /// 能抹掉这笔钱（最好一次 5860 页 → 239 页），但本机 THP 是 `madvise` 模式且
    /// `nr_hugepages=0`，复测就不稳定（4838 页），不能指望。
    ///
    /// 三条否定结论（都实测过，别重复试）：
    ///
    /// - `map(|i| i + base)` 与 `extend_from_slice` 同速 ⇒ 索引改写没有优化空间；
    /// - u32 索引只减字节、不减这条路的时间（按页计费，缺页数不变）；
    /// - **先 `reserve` 再 append 没有可测收益**——std 的 `Vec::append`/`extend`
    ///   内部本来就是"先 reserve 再拷"，显式写出来（两种分配器各 3 次）时间与缺页
    ///   数完全一致；反过来 `reserve_exact` 会削掉摊还余量，让紧随的第一次 `push`
    ///   再付一次全量搬运（实测 3.9→7.5 ms、15.5→34.0 ms，**翻倍**）
    fn append(&mut self, other: &mut Self);

    fn data(&self, slot: usize) -> &MaybeUninit<T>;
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T>;
    /// 槽位当前是否空闲。**只看 `prev` 的最高位**，不读 `data`（空闲槽的 `data`
    /// 是未初始化的，碰它就是 UB）。
    fn is_free(&self, slot: usize) -> bool;

    /// 把槽位标记成空闲（挂回 free 链时调用）。重复标记是幂等的。
    fn mark_free(&mut self, slot: usize);

    /// 槽位的前驱下标。**返回值已掩掉空闲标记位**。
    fn prev(&self, slot: usize) -> usize;
    fn next(&self, slot: usize) -> usize;
    fn set_prev(&mut self, slot: usize, value: usize);
    fn set_next(&mut self, slot: usize, value: usize);

    #[doc(hidden)]
    fn layout(&mut self) -> Layout;
}

// ============================================================
// SoA：三个独立 Vec
// ============================================================

pub struct Soa<T> {
    data: Vec<MaybeUninit<T>>,
    prev: Vec<usize>,
    next: Vec<usize>,
}

impl<T> Soa<T> {
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
        }
    }
}

impl<T> Default for Soa<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> sealed::Sealed for Soa<T> {}

impl<T> Storage<T> for Soa<T> {
    #[inline]
    fn slots(&self) -> usize {
        self.data.len()
    }

    fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
        self.prev.reserve(additional);
        self.next.reserve(additional);
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        self.data.push(MaybeUninit::uninit());
        // 哑元：指向自己的自环（合法下标，见 trait 里的说明）
        self.prev.push(slot);
        self.next.push(slot);

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        // data 没有下标要修 ⇒ 直接整块搬（会搬空 other.data）
        self.data.append(&mut other.data);
        // 两个索引数组一边复制一边加：只写一遍
        self.prev.extend(other.prev.iter().map(|&slot| slot + base));
        self.next.extend(other.next.iter().map(|&slot| slot + base));

        other.prev.clear();
        other.next.clear();
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(slot) }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(slot) }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        unsafe { *self.prev.get_unchecked(slot) & FREE_BIT != 0 }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) |= FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        // 掩掉空闲标记位：调用方要的是下标
        unsafe { *self.prev.get_unchecked(slot) & !FREE_BIT }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { *self.next.get_unchecked(slot) }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) = value };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { *self.next.get_unchecked_mut(slot) = value };
    }

    fn layout(&mut self) -> Layout {
        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: self.prev.as_ptr() as *const u8,
            prev_stride: size_of::<usize>(),
            next: self.next.as_ptr() as *const u8,
            next_stride: size_of::<usize>(),
        }
    }
}

// ============================================================
// Packed：data 一个 Vec，prev/next 打包成 Link
// ============================================================

#[derive(Clone, Copy)]
struct Link {
    prev: usize,
    next: usize,
}

pub struct Packed<T> {
    data: Vec<MaybeUninit<T>>,
    links: Vec<Link>,
}

impl<T> Packed<T> {
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl<T> Default for Packed<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> sealed::Sealed for Packed<T> {}

impl<T> Storage<T> for Packed<T> {
    #[inline]
    fn slots(&self) -> usize {
        self.links.len()
    }

    fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
        self.links.reserve(additional);
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        self.data.push(MaybeUninit::uninit());
        self.links.push(Link {
            prev: slot,
            next: slot,
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        self.data.append(&mut other.data);
        // 两条链接打包在同一个数组里，也只需要写一遍
        self.links.extend(other.links.iter().map(|link| Link {
            prev: link.prev + base,
            next: link.next + base,
        }));

        other.links.clear();
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(slot) }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(slot) }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        unsafe { self.links.get_unchecked(slot).prev & FREE_BIT != 0 }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev |= FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        unsafe { self.links.get_unchecked(slot).prev & !FREE_BIT }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.links.get_unchecked(slot).next }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev = value };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).next = value };
    }

    fn layout(&mut self) -> Layout {
        let links = self.links.as_ptr() as *const u8;

        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: unsafe { links.add(offset_of!(Link, prev)) },
            prev_stride: size_of::<Link>(),
            next: unsafe { links.add(offset_of!(Link, next)) },
            next_stride: size_of::<Link>(),
        }
    }
}

// ============================================================
// AoS：全部字段放进一个 Node
// ============================================================

struct Node<T> {
    data: MaybeUninit<T>,
    prev: usize,
    next: usize,
}

pub struct Aos<T> {
    nodes: Vec<Node<T>>,
}

impl<T> Aos<T> {
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<T> Default for Aos<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> sealed::Sealed for Aos<T> {}

impl<T> Storage<T> for Aos<T> {
    #[inline]
    fn slots(&self) -> usize {
        self.nodes.len()
    }

    fn reserve(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.nodes.len();

        self.nodes.push(Node {
            data: MaybeUninit::uninit(),
            prev: slot,
            next: slot,
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        // data 和两条链接在同一个数组里，"一遍过"就意味着 data 也要逐元素搬
        // （丢掉了 memcpy）；这里按最省事的方式写，让编译器 best-effort。
        self.nodes.extend(other.nodes.drain(..).map(|node| Node {
            data: node.data,
            prev: node.prev + base,
            next: node.next + base,
        }));
    }

    #[inline]
    fn data(&self, slot: usize) -> &MaybeUninit<T> {
        unsafe { &self.nodes.get_unchecked(slot).data }
    }

    #[inline]
    fn data_mut(&mut self, slot: usize) -> &mut MaybeUninit<T> {
        unsafe { &mut self.nodes.get_unchecked_mut(slot).data }
    }

    #[inline]
    fn is_free(&self, slot: usize) -> bool {
        unsafe { self.nodes.get_unchecked(slot).prev & FREE_BIT != 0 }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev |= FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).prev & !FREE_BIT }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).next }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev = value };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).next = value };
    }

    fn layout(&mut self) -> Layout {
        let nodes = self.nodes.as_ptr() as *const u8;

        Layout {
            // 基址是节点数组本身，`data_offset` 再把地址挪到 `data` 字段
            data: self.nodes.as_mut_ptr() as *mut u8,
            data_stride: size_of::<Node<T>>(),
            data_offset: offset_of!(Node<T>, data),
            prev: unsafe { nodes.add(offset_of!(Node<T>, prev)) },
            prev_stride: size_of::<Node<T>>(),
            next: unsafe { nodes.add(offset_of!(Node<T>, next)) },
            next_stride: size_of::<Node<T>>(),
        }
    }
}
