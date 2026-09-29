//! 三种内存布局（`SoaList` / `PackedList` / `AosList`）与
//! std `LinkedList`、`VecDeque`、fast-list、`Vec` 的横向基准。
//!
//! 三种布局是同一个泛型类型 `List<T, S>` 的不同 `Storage` 参数，所以
//! 基准体用宏生成三种实例，不手写三份；基线各自手写。
//!
//! 运行：`cargo bench [-- <过滤正则>]`
//! 报告：`target/criterion/report/index.html`
//!
//! # `append` 这一组怎么读
//!
//! 两个 API 的计费单位不同，**这张表就是选哪个 API 的依据**：
//!
//! - `append`（对齐 `LinkedList::append`）：整块搬对方**全部槽位**（下标一边搬一边
//!   加偏移），按 **对方槽位数 + 自己存储规模** 付账，与对方活元素数无关。
//! - `append_elementwise`（本库扩展）：逐元素搬，优先填自己**已有的空闲槽**，
//!   按 **活元素数** 付账。
//!
//! ## 1M ⊕ 1M（24 MB 载荷）：整块搬的代价几乎全在"目标要不要新页"
//!
//! 那是分配器/内核的行为，不是我们代码的行为（本机每页首次触碰 ~1.2 µs）：
//!
//! | 环境 | SoaList | PackedList | AosList | 计时区缺页 |
//! |---|---|---|---|---|
//! | mimalloc（默认 feature） | 3.7 ~ 4.3 ms | 3.6 ~ 4.0 ms | 3.4 ~ 3.7 ms | 0 |
//! | 系统 malloc（`--no-default-features`） | 8.2 ms | 17.0 ms | 11.3 ms | 4095 / 8095 / 12003 |
//! | 系统 malloc + `GLIBC_TUNABLES=glibc.malloc.hugetlb=1` | 2.8 ms | 4.2 ms | 2.4 ms | 8 / 17 / 24 |
//! | 对照：`mmap` 24 MB、每页只写 1 字节、**不做拷贝** | 8.2 ~ 9.4 ms | — | — | 5860 |
//!
//! 读法：想读"算法本身有多快"就看 mimalloc 那行（0 缺页 = 纯拷贝 + 下标改写 +
//! `realloc` 搬旧数据）；比较不同版本时**必须同时看缺页数**，否则量的是环境。
//!
//! ## 为什么整块 `append` 比 `Vec::append` 慢 2.6 倍：字节对账（mimalloc，1M⊕1M）
//!
//! | 形态 | 时间 | 实搬字节 | 有效带宽 |
//! |---|---|---|---|
//! | `SoaList::append`（3×usize，需扩容） | 3.84 ms | ~96 MB | 25.0 GB/s |
//! | `Vec<usize>::append`（需扩容） | 1.45 ms | ~32 MB | 22.1 GB/s |
//! | 裸 Vec×(usize, u32, u32)（需扩容） | 2.82 ms | ~64 MB | 22.7 GB/s |
//! | 裸 Vec×3 usize（目标预置容量，不扩容） | 1.93 ms | ~48 MB | 24.8 GB/s |
//! | `Vec<usize>`（目标预置容量，不扩容） | 0.60 ms | ~16 MB | 26.6 GB/s |
//! | 热目标 8 MB `extend_from_slice` | 0.58 ms | ~16 MB | 27.4 GB/s |
//! | 热目标 8 MB `extend(map(|i| i + base))` | 0.52 ms | ~16 MB | 30.9 GB/s |
//!
//! - **下标改写免费**：`extend(map(|i| i + base))` 与 `extend_from_slice` 同速（甚至更快，
//!   省掉 memcpy 库调用）⇒ 加偏移不是开销来源。
//! - 差距全是**搬的字节数**：每槽 24 字节（data+prev+next，三个数组）对 `Vec` 的 8 字节；
//!   而两边都因为扩容把每个字节搬两遍（`realloc` 搬旧数据 + 追加拷贝）= 4× 载荷。
//!   所有测点落在同一带宽区间（21~31 GB/s），我们 per-byte 甚至略好。
//! - 想追平只有两条（都实测）：索引降到 u32 ⇒ 16 B/槽、**−37%**；目标预置容量不扩容
//!   ⇒ **−57%**（后者要求分配器保持页热：mimalloc 有效，系统 malloc 下反而更贵）。
//!
//! ## 迭代（`iteration` / `blob_iter`）这一组：瓶颈在"沿链走"，不在迭代器
//!
//! 1M × `usize`，把迭代拆成三层（内部探针，min/9 轮，ns/元素）：
//!
//! | 布局 | 公开 `iter()` | 自己沿链走 | 顺序扫槽位（不碰链） |
//! |---|---|---|---|
//! | SoaList | 1.08 | 1.14 | **0.062** |
//! | PackedList | 1.31 | 1.37 | **0.063** |
//! | AosList | 1.29 | 1.34 | **0.478** |
//! | `Vec` 顺序扫（对照） | — | — | 0.062 |
//!
//! - **迭代器本身没有浪费**：`iter()` 只比"自己沿链走"慢 ~5%（双层判断 + 计数）。
//! - **顺序扫槽位：Soa/Packed 和 `Vec` 同速**（0.062 ns/元素，能按 8 B 步长流式走）；
//!   **Aos 退化 7.7×**——节点交错后每 24 B 只用 8 B，既丢带宽又没法窄步长向量化。
//! - **真正贵的是"沿链走"**：~1.1 ns/元素，本质是"读 `next` → 去那个槽位"的依赖链；
//!   它和 `LinkedList`（1.00 ns/元素）同量级：我们省掉了每节点的独立分配，但
//!   `data` 与 `next` 分在两个数组 ⇒ 每元素多一次加载，正好抵消。
//! - 三种布局各占一头：Soa/Packed 顺序扫最优、沿链要多载入一次；Aos 沿链同线、
//!   顺序扫退化。**还没试的方向**：分组/分块布局（几个元素一组，组内 `data` 连续、
//!   链接连续），同时吃"顺序扫可向量化"和"链走同线"两头。
//! - 这些数字依赖链的局部性（这里是 `push_back` 构造 ⇒ 链就是 0,1,2,… 递增，硬件
//!   预取猜得中）；`random_remove_insert` 之后链会碎，沿链走的成本只会更差。
//!
//! ## 本机的噪声底：小于 ~5% 的差异别当真
//!
//! 用 `jj workspace` 把同一份代码的另一版检出到别的目录、两版**交替**跑同一组基准
//! （3 轮正序 + 2 轮逆序）之后看到：**外部基线自己就会漂**——一次单程全量对比里
//! `Vec` 慢 8.0%、`VecDeque` +5.9%、`LinkedList` +8.6%（全量里最大到 +18%），而同一批
//! 中我们自己的项都在 ±4% 内。也就是说这台机器上"大分配类"基准（`append` / `blob_*`）
//! 的噪声底约 **5 ~ 10%**。
//!
//! 判定回归的三条纪律：① 两版**交替**跑（别让某一版总在同一轮的第二个跑）；
//! ② 同时看外部基线漂了多少，用它当对照；③ 差值得超出噪声底才算数。确定性指标
//! （计时区缺页数，见上文）比墙钟稳，但本机没有 `perf` 可看指令数。
//!
//! ## 大载荷（`Blob64`，64 B/元素，`LARGE_N` = 250k）：`append_blob` 这一组
//!
//! 与 `append` 同形态（两边都由 push/collect 构造 ⇒ 目标要扩容）、同基线，只把元素
//! 换成 64 B。mimalloc 实测（每槽 64 + 8 + 8 = 80 B，载荷 20 MB）：
//!
//! | 形态 | 时间 | 实搬 | 带宽 |
//! |---|---|---|---|
//! | SoaList | 2.87 ~ 3.20 ms | ~80 MB | 25.0 GB/s |
//! | PackedList | 2.86 ~ 2.94 ms | ~80 MB | 28.0 GB/s |
//! | AosList | 2.80 ~ 2.92 ms | ~80 MB | 28.6 GB/s |
//! | VecDeque | 2.22 ms | ~64 MB | 28.8 GB/s |
//! | `Vec`（基线） | 1.98 ~ 2.40 ms | ~64 MB | 26.7 GB/s |
//! | `LinkedList`（O(1) 指针拼接） | 144 ns | 0 | — |
//! | 对照：裸 3 数组预置容量（不扩容） | 1.17 ms | ~40 MB | 34.2 GB/s |
//!
//! - **载荷大了差距变小**：8 B 元素时我们比 `Vec` 慢 2.6×，64 B 时只慢 ~1.2 ~ 1.35×
//!   ——每槽多出的 16 B 链接在大载荷下只占 20%，在 8 B 载荷下占 200%。
//! - **布局差异这时才看得见，但仍然很小**：AosList（1 个数组 ⇒ 1 次 realloc + 1 次
//!   救援拷贝）≈ PackedList（2）≲ SoaList（3），约 5 ~ 12%，落在 criterion 中位数
//!   噪声（±3%）边缘——所以要读"实搬字节 / 带宽"，不要只读墙钟时间。
//! - **省字节的手段在大载荷下收益变小**：索引换 u32 只省 ~10%（8 B 元素时是 37%）；
//!   "目标预置容量、不扩容"依旧是最大的一条（−50 ~ 60%）。
//! ### 同一组里 `append_elementwise` 的表现（目标预先留 `LARGE_N` 个空闲槽）
//!
//! 下面这张表是**同一目标形态下**两条路径的对照（探针 min/9 轮）；基准组里的
//! `SoaList` / `PackedList` / `AosList` 三行用的是"满目标"（要扩容）形态，别和
//! `::elementwise` 三行直接比。mimalloc、250k 元素、每槽 80 B：
//!
//! | 形态 | `append_elementwise` | `append` |
//! |---|---|---|
//! | 目标有 250k 空闲槽 + 源 250k 密 | **1.78 ms** | 3.83 ms |
//! | 目标有 250k 空闲槽 + 源稀疏（250k 槽 / 100 活） | **0.001 ms** | 3.59 ms |
//! | 目标 0 空闲槽 + 源 250k 密 | 3.07 ms | **2.85 ms** |
//! | 目标 0 空闲槽 + 源稀疏 | **0.001 ms** | 2.40 ms |
//!
//! 逐元素的成本随载荷近似线性：**≈ 2 ns + 0.08 ns × 载荷字节 / 元素**（它每个元素都要
//! "出源、进目标"搬两次，且不是流式带宽）——实测 8 B：2.1 ns、64 B：6.9 ns、
//! 256 B：22.4 ns。整块则只按字节走带宽（21 ~ 28 GB/s），与元素个数无关。
//! 所以载荷越大，"目标装不下"那一格两条路越接近（64 B 差 17%，256 B 只差 5%），
//! 而"目标装得下 / 源稀疏"两格逐元素依旧领先（后者稳定领先三个数量级）。
//!
//! criterion 复核（`append_blob` 组、满目标 vs 有空闲槽目标）：我们三种布局的
//! `::elementwise` 是 1.73 / 1.92 / 1.90 ms，而**同组基线 `Vec` 是 1.89 ms、
//! `VecDeque` 2.45 ms** ⇒ 大载荷 + 目标有空闲槽时，`append_elementwise` 不申请内存、
//! 只搬活元素，能跑进 `Vec::append` 的水平。
//!
//! - 数量级对照：std `LinkedList::append` 是 O(1) 指针拼接（144 ns、与载荷无关）。
//!   我们为了迭代局部性（槽位连续）放弃了这一点，这是设计选择。
//!
//! ## 两个 API × 输入形态（目标固定 1M 活元素，N = 1M，min/3 轮，分配器已预热）
//!
//! | 目标空闲槽 \ 源 | 密 1M | 稀疏 1M 槽 / 100 活 | 密 100 |
//! |---|---|---|---|
//! | 0 | 逐元素 4.9 / **整块 3.5** → 用 `append` | 逐元素 **0.001** / 整块 3.4 → **必须用 `append_elementwise`** | ~0 / ~0 |
//! | 1 000 | 逐元素 4.7 / **整块 3.5** → `append` | **逐元素 0.001** / 整块 4.1 | ~0 / ~0 |
//! | 1M | **逐元素 2.1** / 整块 4.9 | **逐元素 0.001** / 整块 4.4 | ~0 / ~0 |
//!
//! 读法：
//! - **逐元素只在"目标已有空闲槽"时便宜**（1M 活元素 2.1 ms，且不产生新页）；目标
//!   装不下时它要边塞边扩容，反而最贵（4.7 ~ 4.9 ms，比整块慢 ~40%）。
//! - **整块按对方槽位数 + 自己存储规模付账**：目标 1M → 3.5 ms，目标 2M（要扩到
//!   3M 槽）→ 4.4 ~ 4.9 ms；对方稀疏时完全不为活元素数打折（100 个活元素照样
//!   3.4 ~ 4.1 ms）⇒ **源稀疏（曾经很大再脱水）时应该用 `append_elementwise`**。
//! - 上表是"页已热"的下限；**冷分配器**下整块会额外付页物化费（系统 malloc 实测
//!   7.7 ~ 8.8 ms / 4095 页），逐元素不受影响（~2.1 ms、0 缺页）。
//!
//! 大页是最有效的旋钮，但都落在库外：
//! - glibc ≥ 2.35：`GLIBC_TUNABLES=glibc.malloc.hugetlb=1`（进程级环境变量，库内
//!   无法开启）——它让 glibc 给大块 `madvise(MADV_HUGEPAGE)`。
//! - mimalloc：默认就复用页（0 缺页）；`MIMALLOC_PURGE_DELAY=-1` 可进一步稳住。
//! - 本机 THP 是 `madvise` 模式且 `nr_hugepages=0`：自己 `mmap` + `MADV_HUGEPAGE`
//!   实测**时好时坏**（好：1.1 ~ 1.4 ms / 239 缺页；坏：11 ~ 16 ms / 5860 缺页，
//!   2 MB 页要不到就静默退化成 4 KB）。`MADV_COLLAPSE` 对**未触碰**的范围无效
//!   （只折叠已填充的页表）。所以库里别指望靠 `madvise` 根治，除非接受"有时生效"。

#![feature(linked_list_cursors)]

// 全局分配器。`cargo bench --no-default-features` 会切回系统 malloc，
// 便于对比"分配器是否把内存还给内核"对端操作的影响。
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use slot_list::{AosList, PackedList, SoaList};

use fast_list::LinkedList as FastLinkedList;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};

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
// 基线：std VecDeque / std LinkedList / fast-list / Vec
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

fn fastlist_push_back_pop_front() {
    let mut list = FastLinkedList::new();

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

fn fastlist_push_front_pop_back() {
    let mut list = FastLinkedList::new();

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

fn make_fastlist() -> FastLinkedList<usize> {
    let mut list = FastLinkedList::new();

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

fn fastlist_iter(list: &FastLinkedList<usize>) {
    let mut sum = 0usize;

    for item in list.iter() {
        sum = sum.wrapping_add(item.value);
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

fn fastlist_at_middle(list: &FastLinkedList<usize>) {
    let mut sum = 0usize;

    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let index = list.nth(black_box(middle)).unwrap();

        let item = list.get(index).unwrap();

        sum = sum.wrapping_add(item.value);
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

fn fastlist_insert_before_remove(list: &mut FastLinkedList<usize>) {
    let middle_index = list.nth(N / 2).unwrap();

    for i in 0..MIDDLE_OPS {
        let inserted = list.insert_before(middle_index, black_box(i));

        let removed = list.remove(inserted).unwrap();

        black_box(removed.value);
    }

    black_box(middle_index);
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

fn fastlist_churn(list: &mut FastLinkedList<usize>) {
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

fn linkedlist_random_remove_insert(list: &mut LinkedList<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let mut cursor = linkedlist_cursor_at(list, pos);

        let value = cursor.remove_current().unwrap();

        black_box(value);

        cursor.insert_before(black_box(i));
    }

    black_box(list.len());
}

fn fastlist_random_remove_insert(list: &mut FastLinkedList<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let index = list.nth(pos).unwrap();

        let removed = list.remove(index).unwrap();

        black_box(removed.value);

        if pos < list.len() {
            let next_index = list.nth(pos).unwrap();

            list.insert_before(next_index, black_box(i));
        } else {
            list.push_back(black_box(i));
        }
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

fn fastlist_index_update(list: &mut FastLinkedList<usize>) {
    let index = list.nth(N / 2).unwrap();

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = list.get(index).unwrap().value;

        let new_value = value.wrapping_add(delta);

        list.get_mut(index).unwrap().value = new_value;

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

fn make_fastlist_blob() -> FastLinkedList<Blob64> {
    let mut list = FastLinkedList::new();

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

fn fastlist_blob_iter(list: &FastLinkedList<Blob64>) {
    let mut sum = 0usize;

    for item in list.iter() {
        sum = sum.wrapping_add(item.value.key());
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

fn fastlist_blob_push_back_pop_front() {
    let mut list = FastLinkedList::new();

    for i in 0..LARGE_N {
        list.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = list.pop_front() {
        black_box(value);
    }
}

// ============================================================
// Criterion groups
// ============================================================

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
    g.bench_function("FastList", |b| b.iter(fastlist_push_front_pop_back));
    g.finish();
}

fn iteration(c: &mut Criterion) {
    let soalist = make_soalist();
    let packedlist = make_packedlist();
    let aoslist = make_aoslist();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let fastlist = make_fastlist();
    let vec = make_vec();

    let mut g = c.benchmark_group("iteration");
    g.bench_function("SoaList", |b| b.iter(|| soalist_iter(&soalist)));
    g.bench_function("PackedList", |b| b.iter(|| packedlist_iter(&packedlist)));
    g.bench_function("AosList", |b| b.iter(|| aoslist_iter(&aoslist)));
    g.bench_function("VecDeque", |b| b.iter(|| vecdeque_iter(&vecdeque)));
    g.bench_function("LinkedList", |b| b.iter(|| linkedlist_iter(&linkedlist)));
    g.bench_function("FastList", |b| b.iter(|| fastlist_iter(&fastlist)));
    g.bench_function("Vec", |b| b.iter(|| vec_iter(&vec)));
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
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

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
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_random_remove_insert(&mut linkedlist, &positions))
    });
    g.bench_function("FastList", |b| {
        b.iter(|| fastlist_random_remove_insert(&mut fastlist, &positions))
    });
    g.finish();
}

fn cursor_update(c: &mut Criterion) {
    let mut soalist = make_soalist();
    let mut packedlist = make_packedlist();
    let mut aoslist = make_aoslist();
    let mut vecdeque = make_vecdeque();
    let mut linkedlist = make_linkedlist();
    let mut fastlist = make_fastlist();

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
    g.bench_function("LinkedList", |b| {
        b.iter(|| linkedlist_cursor_update(&mut linkedlist))
    });
    g.bench_function("FastList", |b| {
        b.iter(|| fastlist_index_update(&mut fastlist))
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
    let fastlist = make_fastlist_blob();
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
    g.bench_function("FastList", |b| b.iter(|| fastlist_blob_iter(&fastlist)));
    g.bench_function("Vec", |b| b.iter(|| vec_blob_iter(&vec)));
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
    g.bench_function("FastList", |b| b.iter(fastlist_blob_push_back_pop_front));
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
}

criterion_main!(benches);
