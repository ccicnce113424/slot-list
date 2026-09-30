//! 内存布局策略：`data` / `prev` / `next` 如何落到内存里。
//!
//! 三种策略的**算法完全相同**，只有存储方式不同：
//!
//! - [`Split`]：三个独立 `Vec`（array of structs 的反面，structure of arrays）
//! - [`PackedLinks`]：`data` 一个 `Vec`，`prev`/`next` 打包成 [`Link`] 交错存放
//! - [`Nodes`]：三者放进一个 `Node`（array of structs）

use alloc::vec::Vec;
use core::mem::{MaybeUninit, offset_of, size_of};

/// 「这一侧没有邻居」。
///
/// **只作为标量使用**：函数参数（`List::insert_link` 的 `prev` / `next`）和
/// 游标的幽灵位置 tag。它**绝不会出现在 `prev` / `next`
/// 数组里**——数组里每个值都是合法槽位下标（链表两端的"哑元"字段是"没人读
/// 的合法下标"，不是 NIL）。这是 [`Storage::append`] 能把整段下标直接加偏移
/// 的前提。
pub(crate) const NIL: usize = usize::MAX;

/// 链接数组里用多宽的整数存槽位下标。
///
/// 三种布局的每槽开销 = `T` + 两条链接，所以链接宽度直接决定内存与访存带宽：
/// `usize` 换 `u32` 时 `T = 8` 的每槽从 24 B 降到 **16 B**（`PERFORMANCE.md` §2/§4）。
///
/// **上限**：空闲标记位占最高位（见 [`Ix::FREE_BIT`]），而 `append` 的整段 `+= base`
/// 靠"不向这一位进位"来免特例，所以可用槽位数是 `1 << (BITS - 1)`：
/// `u32` ⇒ 2.1G、`u16` ⇒ 32768、`u8` ⇒ 128（默认宽度见 [`DefaultIx`]）。超过就在 `grow` / `reserve`
/// 处 panic，不会静默截断。
///
/// 这个 trait 是**封闭**的（`u8` ~ `u64` / `usize`），出现在类型参数的位置上。
pub trait Ix:
    Copy
    + Eq
    + core::ops::Add<Output = Self>
    + core::ops::BitOrAssign
    + core::ops::BitAnd<Output = Self>
    + sealed::Sealed
    + 'static
{
    /// 宽度（位）。
    const BITS: u32 = size_of::<Self>() as u32 * 8;

    /// 可用的槽位数上限（= 空闲标记位本身 ⇒ 索引加 `base` 不会进位到标记位）。
    const MAX_SLOTS: usize = 1 << (Self::BITS - 1);

    /// 「这个槽位当前空闲」的标记位，存在 `prev` 的**最高位**（下面以 `usize` 为例，
    /// 窄索引同理，位宽换成 `Ix::BITS - 1`）。
    ///
    /// 为什么要它：句柄（[`Slot`](crate::Slot)）是指向槽位的裸标识，槽位会被 free-list
    /// 复用 ⇒ 必须能 O(1) 判断"这个槽位现在活没活"，**否则就要对空闲槽的
    /// `MaybeUninit` 做 `assume_init_*`（UB）**。
    ///
    /// 为什么放在 `prev` 的最高位：
    ///
    /// - **写入最省**：live 槽位的 `prev` 由链接写入覆盖（那些值天然没有这一位），空闲槽位
    ///   只被 free 链用到 `next` ⇒ 这一位**只在"变空闲"那一次**写，链接写入路径上没有任何
    ///   额外操作（实测：把这次写改成**直接赋值** `prev = FREE_BIT` 想省下一次 load，
    ///   反而让 `churn` 稳定慢 7~8%——两轮交替 A/B、对照 `VecDeque` 持平——所以保留读-改-写）；
    /// - **`append` 的整段 `+= base` 自己会带着它走**：`2^63 | v` 加 `base` 仍是
    ///   `2^63 | (v + base)`，只要不溢出 `u64` 就不需要特例（`v + base < 2^63`，而
    ///   槽位数远小于此 ⇒ 实际永远成立，`debug_assert` 兜底）；
    /// - **读 `prev` 只在 live 槽位上**（这是契约，不是建议）：因为空闲槽的 `prev` 就是
    ///   这个标记位本身（`append` 后是 `FREE_BIT | base`），掩码反而要每个读取点多付一条
    ///   and、还拖长"取地址"的依赖链。所以 [`Storage::prev`] **直接返回原值**，并在
    ///   debug 构建里用 `debug_assert` 钉住"调用它的槽位必须是 live"（release 零成本）。
    ///
    /// # 标记空闲怎么写：三种写法都量过
    ///
    /// 这个位**只在"变空闲"那一次**写，所以它的写法直接落在 `churn` 的热路径上。
    ///
    /// | 写法 | `churn`（Split / PackedLinks，两轮交替 A/B） | 结论 |
    /// |---|---|---|
    /// | `prev |= FREE_BIT`（当前） | **2.79 / 2.76 ms** | 保留 |
    /// | `prev = FREE_BIT` | 2.99~3.04 / 2.96~2.98（**+7%**） | 否 |
    /// | 只写标志位那个字节 | 2.95~3.00 / 2.96（**+5.5~7.5%**） | 否 |
    ///
    /// 三种都查了汇编，但**片段级的汇编不可外推**——真实热循环里 LLVM 的选择完全不同：
    ///
    /// ```text
    /// 片段里的 |=        orb    $-128, 7(%rdi,%rsi,8)      1 条 / 5 字节（含 load）
    /// 片段里的 =         movabsq $-9223372036854775808, %rax
    ///                    movq   %rax, (%rdi,%rsi,8)        2 条 / 14 字节
    /// 片段里的字节写      movb   $-128, 7(%rdi,%rsi,8)      1 条 / 5 字节（无 load）
    ///
    /// crate 里真实的 |=  movabs $0x8000000000000000,%r9     ← 序言里，**提出循环**
    ///                    or     %r9,(%rsi,%r10,8)          ← 循环内，**寄存器形式** RMW
    /// crate 里真实的字节写 movb  $0x80,0x7(%rsi,%r9,8)      ← 循环内，且**没有 movabs**
    /// ```
    ///
    /// 把整个 `churn` 函数逐条 diff（去掉地址与分支目标）还能看到：换写法**远不止那一条指令**——
    /// 寄存器分配整体重排（常量不再占一个寄存器）、栈帧从 `push %rbp` 变 `sub $0x10,%rsp`、
    /// 指令数 133 → 126、**连循环的对齐填充都变了**（`cs nopw` → `data16` 前缀的 nop；热块入口
    /// 从 `mod 64 = 0` 挪到 `mod 64 = 60`）。所以那 5.5~7.5% **不能归给标志位那一条指令**。
    ///
    /// 布局扰动实验（插一个永不调用、只用来挪热函数位置的 `pub #[inline(never)]` 函数）量到的
    /// 噪声底是 **±2%**；上面那几个 7% 里至少有相当一部分来自"换写法顺带把热循环摆到别处"。
    ///
    /// 结论仍保留 `|=`（本机实测最快也最稳），但**换 ISA 应当重测**：IR 层面只写一个字节确实
    /// 少一次 load（`--emit llvm-ir` 实测：`store i8 -128` 对 `load i64` + `or` + `store`），
    /// RISC 后端会从 `ldrb`/`orr`/`strb` 三条降到一条。
    ///
    /// 复现方法：写一个 20 行的 `#[inline(never)]` churn 探针（`pop_front` + `push_back` 循环）
    /// 编成 example，两种写法各 build 一次，`objdump -d` 去掉地址后 `diff`。
    ///
    /// 反过来，把它当**布尔**用（`raw & FREE_BIT != 0`）没有这个问题：LLVM 会化成
    /// `mov %rdi,%rax; shr $63,%rax`（2 条指令、无立即数）。
    ///
    /// 代价：每个"变成空闲"的槽位一次 read-modify-write（`pop`/`remove`/`clear` 等
    /// 每条回收路径一次）。
    const FREE_BIT: Self;

    /// 零（用来测标记位，避免依赖 `PartialEq<{integer}>`）。
    const ZERO: Self;

    fn to_usize(self) -> usize;

    fn from_usize(value: usize) -> Self;
}

/// 触顶的冷路径。
///
/// **故意** `#[cold] #[inline(never)]` + 消息不带格式参数：`grow` 是热路径的一部分
/// （`alloc_slot` 要内联进来）。带上 `{}` 参数就会把格式化机器拖进 `grow`，内联器随即
/// 放弃内联 `alloc_slot`——实测 `churn` 因此慢 47%（循环里出现 `call`）。
#[cold]
#[inline(never)]
fn ix_overflow() -> ! {
    panic!("槽位数超出索引宽度上限（见 Ix::MAX_SLOTS）")
}

macro_rules! impl_ix {
    ($($t:ty),* $(,)?) => {$(
        impl sealed::Sealed for $t {}

        impl Ix for $t {
            const FREE_BIT: Self = 1 << (<$t>::BITS - 1);
            const ZERO: Self = 0;

            #[inline]
            fn to_usize(self) -> usize {
                self as usize
            }

            #[inline]
            fn from_usize(value: usize) -> Self {
                debug_assert!(
                    value < Self::MAX_SLOTS,
                    "槽位下标超出所选索引宽度：{value} >= {}",
                    Self::MAX_SLOTS
                );

                value as $t
            }
        }
    )*};
}

impl_ix!(u8, u16, u32, u64, usize);

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
/// - `prev_stride` / `next_stride`：相邻元素的链接字段相隔多少字节——PackedLinks 里
///   两个链接打包成 `Link`，Nodes 里它们和 `data` 同处一个 `Node<T>`，所以这里的
///   步长是"元素"大小而不是 8。
///
/// 三个布局具体怎么落到这些字段上（`T = usize`、索引 `u32`；括号里是
/// `T = [u64; 8]`，实测值）：
///
/// | 布局 | `data` 基址 / `data_stride` / `data_offset` | `prev`·`next` 基址 / 步长 |
/// |---|---|---|
/// | `Split<T, I>` | 数据数组首址 / `size_of::<T>()` 8（64）/ 0 | 各自数组首址 / `size_of::<I>()` 4 |
/// | `PackedLinks<T, I>` | 数据数组首址 / 8（64）/ 0 | `links + offset_of!(Link<I>, …)` 0·4 / 8 |
/// | `Nodes<T, I>` | 节点数组首址 / `size_of::<Node<T, I>>()` 16（72）/ `offset_of!(Node<T, I>, data)` 0 | `nodes + offset_of!(Node<T, I>, …)` 8·12 / 16（72） |
///
/// ```text
/// Split      data  [d0][d1][d2]…      prev [p0][p1]…      next [n0][n1]…
///                 ↑ stride 8               ↑ stride 4           ↑ stride 4
/// PackedLinks   data  [d0][d1]…          links [(p0,n0)][(p1,n1)]…
///                                           ↑ prev=links+0, next=links+4, stride 8
/// Nodes      nodes [(d0,p0,n0)][(d1,p1,n1)]…
///                  ↑ data=nodes+0, prev=nodes+8, next=nodes+12, stride 16
/// ```
///
///「两个索引域」：`data` 域按 `Layout` 的步长走（宽度由布局决定），**链接域一律是
/// `Ix` 宽度**（默认 8 字节，见 [`DefaultIx`]）——[`IterMut`](crate::IterMut) 靠 `ix_width` 决定怎么读。
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
    /// 链接元素的字节宽度（1/2/4/8）：迭代器按裸地址读链接时要按它来读 + 加宽。
    pub(crate) ix_width: usize,
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

    /// 把底层 `Vec` 的**多余容量**还给分配器。**槽位数不变**（空闲槽是 free 链的一部分，
    /// 也是句柄指向的东西，不能丢）⇒ 不变量、`Slot` 句柄、链结构全都不动。
    fn shrink_to_fit(&mut self) {
        // 默认：无操作（自定义布局可以不支持）
    }

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
    ///
    /// 实现里就地把 `prev` 的最高位（[`Ix::FREE_BIT`]）置上，不做读-改-写之外的任何事——
    /// `append` 的整段 `+= base` 会把它变成 `FREE_BIT | base`，标记位仍在。
    fn mark_free(&mut self, slot: usize);

    /// 槽位的前驱下标。
    ///
    /// **契约：只在 `!is_free(slot)` 的槽位上调用**（空闲槽的 `prev` 就是空闲标记位本身，
    /// 没有意义）。因此这里**不做掩码**——掩码会让每个读取点多一条 and、并拖长取地址的
    /// 依赖链；越界误用由 debug 构建的 `debug_assert` 抓住。
    fn prev(&self, slot: usize) -> usize;
    fn next(&self, slot: usize) -> usize;
    fn set_prev(&mut self, slot: usize, value: usize);
    fn set_next(&mut self, slot: usize, value: usize);

    #[doc(hidden)]
    fn layout(&mut self) -> Layout;
}

// ============================================================
// 默认索引宽度
// ============================================================

/// 索引宽度默认值：**`usize`**。
///
/// 开 `u32-index` feature 换成 `u32`（每槽省 1/3、`append` 快 37%，上限见
/// [`Ix::MAX_SLOTS`]）——那是"在窄索引下跑全套测试/基准"的开关，**默认关闭**。
#[cfg(feature = "u32-index")]
pub type DefaultIx = u32;
#[cfg(not(feature = "u32-index"))]
pub type DefaultIx = usize;

// ============================================================
// Split：数据、前驱、后继三条流各自成数组
// ============================================================

/// **三条流完全分开**：`data` / `prev` / `next` 各一个 `Vec`。
///
/// 走链只碰索引数组（`u32` 时 4 B/槽），带宽最省；代价是每个槽位三个基址，
/// 元素与链接永远不在同一条 cache line。
///
/// 索引宽度可调（`prev`/`next` 一起窄）：`Split<T, u16>` / `Split<T, u32>` /
/// `Split<T, usize>`，默认见 [`DefaultIx`]。
pub struct Split<T, I = DefaultIx> {
    data: Vec<MaybeUninit<T>>,
    prev: Vec<I>,
    next: Vec<I>,
}

impl<T, I: Ix> Split<T, I> {
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
        }
    }
}

impl<T, I: Ix> Split<T, I> {
    /// 裸部件（只读）：`(data, prev, next)`，三者长度相等 = 槽位数。
    ///
    /// **这是"状态可搬运"的入口**：整条链的全部状态 = 这些数组 + 那五个数字
    /// （[`List::into_raw`](crate::List::into_raw)），序列化 / 落盘 / 共享内存都从这里取。
    pub fn as_parts(&self) -> (&[MaybeUninit<T>], &[I], &[I]) {
        (&self.data, &self.prev, &self.next)
    }

    /// 裸部件（拿走所有权），配合 [`Self::from_parts`]。
    pub fn into_parts(self) -> (Vec<MaybeUninit<T>>, Vec<I>, Vec<I>) {
        (self.data, self.prev, self.next)
    }

    /// 从裸部件装回去。
    ///
    /// # Safety
    ///
    /// 调用者保证：三个数组长度相等；每个 `prev`/`next` 都是**合法槽位下标**
    /// （不变量：`NIL` 从不写进数组，两端哑元是自环）；**空闲槽的 `prev` 最高位是
    /// 空闲标记**（[`Ix::FREE_BIT`]），否则 `is_free` 判错、`assume_init_drop` 会踩
    /// 未初始化数据；live 槽的 `data` 必须已初始化。不满足是 UB，不会 panic。
    pub unsafe fn from_parts(data: Vec<MaybeUninit<T>>, prev: Vec<I>, next: Vec<I>) -> Self {
        debug_assert_eq!(data.len(), prev.len(), "from_parts: data/prev 长度不等");
        debug_assert_eq!(data.len(), next.len(), "from_parts: data/next 长度不等");

        Self { data, prev, next }
    }
}

impl<T, I: Ix> Default for Split<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for Split<T, I> {}

impl<T, I: Ix> Storage<T> for Split<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.data.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.data.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.data.reserve(additional);
        self.prev.reserve(additional);
        self.next.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.prev.shrink_to_fit();
        self.next.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.data.push(MaybeUninit::uninit());
        // 哑元：指向自己的自环（合法下标，见 trait 里的说明）
        self.prev.push(I::from_usize(slot));
        self.next.push(I::from_usize(slot));

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        // 先一次性确认整体不越界（这样下面的逐元素加基址就不会向标记位进位）
        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

        // data 没有下标要修 ⇒ 直接整块搬（会搬空 other.data）
        self.data.append(&mut other.data);
        // 两个索引数组一边复制一边加：只写一遍。
        // **在窄整数域里加**，不能过 `from_usize`：空闲槽的 `prev` 还带着空闲标记位
        // （`FREE_BIT | v`），加上基址之后仍是 `FREE_BIT | (v + base)` —— 这正是
        // `prev` 不需要单独修标记位的原因（见 `FREE_BIT` 的文档）。
        self.prev.extend(other.prev.iter().map(|&ix| ix + base));
        self.next.extend(other.next.iter().map(|&ix| ix + base));

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
        // 当**布尔**用时不必担心立即数：LLVM 会把"与最高位"化成一次移位
        // （实测 2 条指令、没有 `movabs`，见 FREE_BIT 的文档）。
        unsafe { (*self.prev.get_unchecked(slot) & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        debug_assert!(!self.is_free(slot), "只能在 live 槽位上读 prev");
        unsafe { self.prev.get_unchecked(slot).to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.next.get_unchecked(slot).to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { *self.prev.get_unchecked_mut(slot) = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { *self.next.get_unchecked_mut(slot) = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: self.prev.as_ptr() as *const u8,
            prev_stride: size_of::<I>(),
            next: self.next.as_ptr() as *const u8,
            next_stride: size_of::<I>(),
            ix_width: size_of::<I>(),
        }
    }
}

// ============================================================
// PackedLinks：data 一个 Vec，prev/next 打包成 Link
// ============================================================

/// `PackedLinks` 布局里一条槽位的两条链接。
///
/// 公开是因为**裸部件 API**（`PackedLinks::as_parts` / `from_parts`）要把它交出去 ——
/// 想自己序列化 / 从共享内存恢复整条链，就需要能读写这个类型。
#[derive(Clone, Copy, Debug)]
pub struct Link<I> {
    /// 前驱槽位下标（空闲槽的最高位是空闲标记，见 [`Ix::FREE_BIT`]）。
    pub prev: I,
    /// 后继槽位下标。
    pub next: I,
}

/// **只有索引成对交错**：`data` 一个 `Vec`，`prev`/`next` 打包进同一个 `Link` 数组。
///
/// 走链时两条链接同处一条 cache line（一次 miss 拿到前驱和后继），`data` 仍独立
/// ⇒ 判空闲标记只碰索引、不碰元素。
///
/// 索引宽度可调（两个字段在同一个 `Link` 里，只能一起窄）。默认见 [`DefaultIx`]。
pub struct PackedLinks<T, I = DefaultIx> {
    data: Vec<MaybeUninit<T>>,
    links: Vec<Link<I>>,
}

impl<T, I: Ix> PackedLinks<T, I> {
    pub const fn new() -> Self {
        Self {
            data: Vec::new(),
            links: Vec::new(),
        }
    }
}

impl<T, I: Ix> PackedLinks<T, I> {
    /// 裸部件（只读）：`(data, links)`，两个数组长度相等 = 槽位数。
    pub fn as_parts(&self) -> (&[MaybeUninit<T>], &[Link<I>]) {
        (&self.data, &self.links)
    }

    /// 裸部件（拿走所有权），配合 [`Self::from_parts`]。
    pub fn into_parts(self) -> (Vec<MaybeUninit<T>>, Vec<Link<I>>) {
        (self.data, self.links)
    }

    /// 从裸部件装回去。
    ///
    /// # Safety
    ///
    /// 要求与 [`Split::from_parts`] 相同：两个数组长度相等；每个 `prev`/`next` 都是合法槽位
    /// 下标；空闲槽的 `prev` 最高位是空闲标记（[`Ix::FREE_BIT`]）；live 槽的 `data` 已初始化。
    pub unsafe fn from_parts(data: Vec<MaybeUninit<T>>, links: Vec<Link<I>>) -> Self {
        debug_assert_eq!(data.len(), links.len(), "from_parts: data/links 长度不等");

        Self { data, links }
    }
}

impl<T, I: Ix> Default for PackedLinks<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for PackedLinks<T, I> {}

impl<T, I: Ix> Storage<T> for PackedLinks<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.links.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.data.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.data.reserve(additional);
        self.links.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
        self.links.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.data.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.data.push(MaybeUninit::uninit());
        self.links.push(Link {
            prev: I::from_usize(slot),
            next: I::from_usize(slot),
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

        self.data.append(&mut other.data);
        // 两条链接打包在同一个数组里，也只需要写一遍（加法在窄整数域里做，
        // 空闲槽的 `prev` 带着标记位，见 FREE_BIT 文档）
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
        unsafe { (self.links.get_unchecked(slot).prev & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        debug_assert!(!self.is_free(slot), "只能在 live 槽位上读 prev");
        unsafe { self.links.get_unchecked(slot).prev.to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.links.get_unchecked(slot).next.to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).prev = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(slot).next = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        let links = self.links.as_ptr() as *const u8;

        Layout {
            data: self.data.as_mut_ptr() as *mut u8,
            data_stride: size_of::<MaybeUninit<T>>(),
            data_offset: 0,
            prev: unsafe { links.add(offset_of!(Link<I>, prev)) },
            prev_stride: size_of::<Link<I>>(),
            next: unsafe { links.add(offset_of!(Link<I>, next)) },
            next_stride: size_of::<Link<I>>(),
            ix_width: size_of::<I>(),
        }
    }
}

// ============================================================
// Nodes：全部字段放进一个 Node
// ============================================================

/// `Nodes` 布局里的一个槽位：元素与两条链接同处一条 cache line。
///
/// 公开的理由同 [`Link`]（裸部件 API）。
pub struct Node<T, I> {
    /// 元素；空闲槽这里是未初始化的（**不要** `assume_init`）。
    pub data: MaybeUninit<T>,
    /// 前驱槽位下标（空闲槽的最高位是空闲标记，见 [`Ix::FREE_BIT`]）。
    pub prev: I,
    /// 后继槽位下标。
    pub next: I,
}

impl<T, I> Node<T, I> {
    /// 组装一个槽位。
    pub fn new(data: MaybeUninit<T>, prev: I, next: I) -> Self {
        Self { data, prev, next }
    }
}

/// **数据与索引整节点交错**：每槽一个 `Node`（`data` + `prev` + `next`），一个 `Vec` 装完。
///
/// `next` 与 `data` 同处一条 cache line（拿到下标顺带有元素）⇒ 短暂访问元素最省；
/// 代价是**只判空闲标记也要按整节点付带宽**（见 [`List::clear`] 的密度表：它的扫描
/// 阈值最高）。
///
/// 索引宽度默认见 [`DefaultIx`]。
pub struct Nodes<T, I = DefaultIx> {
    nodes: Vec<Node<T, I>>,
}

impl<T, I: Ix> Nodes<T, I> {
    pub const fn new() -> Self {
        Self { nodes: Vec::new() }
    }
}

impl<T, I: Ix> Nodes<T, I> {
    /// 裸部件（只读）：`nodes`（每个节点自带元素与两条链接）。
    pub fn as_parts(&self) -> &[Node<T, I>] {
        &self.nodes
    }

    /// 裸部件（拿走所有权），配合 [`Self::from_parts`]。
    pub fn into_parts(self) -> Vec<Node<T, I>> {
        self.nodes
    }

    /// 从裸部件装回去。
    ///
    /// # Safety
    ///
    /// 要求与 [`Split::from_parts`] 相同：每个节点里的 `prev`/`next` 都是合法槽位下标；
    /// 空闲槽的 `prev` 最高位是空闲标记（[`Ix::FREE_BIT`]）；live 槽的 `data` 已初始化。
    pub unsafe fn from_parts(nodes: Vec<Node<T, I>>) -> Self {
        Self { nodes }
    }
}

impl<T, I: Ix> Default for Nodes<T, I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, I: Ix> sealed::Sealed for Nodes<T, I> {}

impl<T, I: Ix> Storage<T> for Nodes<T, I> {
    #[inline]
    fn slots(&self) -> usize {
        self.nodes.len()
    }

    fn reserve(&mut self, additional: usize) {
        if let Some(total) = self.nodes.len().checked_add(additional)
            && total > I::MAX_SLOTS
        {
            ix_overflow();
        }

        self.nodes.reserve(additional);
    }

    fn shrink_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
    }

    #[inline]
    fn grow(&mut self) -> usize {
        let slot = self.nodes.len();

        if slot >= I::MAX_SLOTS {
            ix_overflow();
        }

        self.nodes.push(Node {
            data: MaybeUninit::uninit(),
            prev: I::from_usize(slot),
            next: I::from_usize(slot),
        });

        slot
    }

    fn append(&mut self, other: &mut Self) {
        let base = self.slots();

        if base + other.slots() > I::MAX_SLOTS {
            ix_overflow();
        }

        let base = I::from_usize(base);

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
        unsafe { (self.nodes.get_unchecked(slot).prev & I::FREE_BIT) != I::ZERO }
    }

    #[inline]
    fn mark_free(&mut self, slot: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev |= I::FREE_BIT };
    }

    #[inline]
    fn prev(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).prev.to_usize() }
    }

    #[inline]
    fn next(&self, slot: usize) -> usize {
        unsafe { self.nodes.get_unchecked(slot).next.to_usize() }
    }

    #[inline]
    fn set_prev(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).prev = I::from_usize(value) };
    }

    #[inline]
    fn set_next(&mut self, slot: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(slot).next = I::from_usize(value) };
    }

    fn layout(&mut self) -> Layout {
        let nodes = self.nodes.as_ptr() as *const u8;

        Layout {
            // 基址是节点数组本身，`data_offset` 再把地址挪到 `data` 字段
            data: self.nodes.as_mut_ptr() as *mut u8,
            data_stride: size_of::<Node<T, I>>(),
            data_offset: offset_of!(Node<T, I>, data),
            prev: unsafe { nodes.add(offset_of!(Node<T, I>, prev)) },
            prev_stride: size_of::<Node<T, I>>(),
            next: unsafe { nodes.add(offset_of!(Node<T, I>, next)) },
            next_stride: size_of::<Node<T, I>>(),
            ix_width: size_of::<I>(),
        }
    }
}
