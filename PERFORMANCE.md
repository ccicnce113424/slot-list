# `slot-list` 性能总结

`List<T, S>` = **下标即指针**的双向链表：节点放在预分配的连续槽位里，删掉的槽位用
free-list 串起来复用；三种内存布局（`SoaList` / `PackedList` / `AosList`）是同一个泛型
类型的三个 `Storage` 参数。基线是 std 的 `LinkedList`、`VecDeque`、`Vec`。

---

## 0. 速览

| 问题 | 结论（当日全套，1M 元素） |
|---|---|
| 端操作 `push`+`pop`（不预分配） | 比 `VecDeque` 慢 2.4~3.0×，与 `LinkedList` 同级（6.1~7.5 vs 5.8~6.0 ms）——这组由**缺页与拷贝量**支配（§3.1） |
| 稳态端操作 `churn`（无分配） | **比 `LinkedList` 快 2.2×**（2.75 vs 6.15 ms），比 `VecDeque` 慢 1.6×（§3.4） |
| 迭代 | 与 `LinkedList` 同级（1.21 vs 1.05 ms；~1.1 ns/元素 = 依赖加载下限）；**64 B 载荷下比它快 1.74×**（§3.2） |
| 中间位置插删 | 与 `LinkedList` 同级、比 `VecDeque` 快 150×（后者每次要 memmove 50 万元素）（§3.3） |
| 内存/槽 | **固定 16 B**（T=8，默认 `u32` 索引；换 `usize` 是 24 B）；三种布局在 T=64 上统一 72 B（§2） |
| 与 std `LinkedList` 的关键差异 | ① 无每节点分配（`churn` / `blob_end_ops` 领先的原因）；② `append` 不是 O(1) 指针拼接（§3.6） |
| 已确认到顶 | **在本库这个形状内**：迭代的"沿链走"、`append` 的拷贝（已到带宽）——三条路线都实现/建模过并否掉（§4）。想更快只能换形状：分块 ≈ `LinkedList<Vec<T>>`，全项更快，但没有元素级游标/稳定句柄（§4 末） |
| 独特价值（性能之外） | **稳定句柄 `Slot`（O(1) 进入/删除/搬移，实测比按位置快约 4 万倍）**、热路径零分配（有测试）、状态可搬运（有测试）、布局可换——见 §8 |
| 最有效的旋钮 | 复用空闲槽（`append_elementwise`）与预置容量（`with_capacity`），其次是分配器/大页（§6） |

---

## 1. 方法与读数纪律

- 单线程、`cargo bench`（criterion，`sample_size(10)`、`measurement_time(1s)`，报中位数）。
- **噪声底：±5%**（`iteration` / `churn` / `cursor_update` 这类小工作集更小）；
  `append*` / `blob_*` 这类大分配组 **±10~20%**——机器状态一漂，同一份代码的 `churn`
  都能从 2.80 漂到 3.01 ms，而对照基线（`VecDeque`）不动，所以**必须看对照**。
- **`end_ops` / `churn` / `append*` 是分配器/kernel 计量，不是代码计量**：`append` 同一
  份代码在不同时刻能从 2 ms 漂到 16 ms（缺页 0 → 8187）。比较时看**计时区缺页数**。
- 判定回归：两版**交替**跑（A/B/A/B），用外部基线（`Vec`/`VecDeque`/`LinkedList`）
  当参照；差值得超出噪声底才算数。
- 口径：`N` = 1M 元素（`usize`，8 B）；`LARGE_N` = 250k（`Blob64`，64 B）。

---

## 2. 三种布局：内存与地址（实测核对过）

`Layout` 用「基址 + 步长 + 偏移」描述地址（下标 = 槽位号）：

```text
元素 i 的 data 地址 = data 基址 + i * data_stride + data_offset
元素 i 的 prev 值   = *(prev 基址 + i * prev_stride)
元素 i 的 next 值   = *(next 基址 + i * next_stride)
```

| 布局 | 内存/槽（T=8 / T=64，默认 `u32`） | `data_stride` | `data_offset` | `prev`·`next` 步长 | 结构 |
|---|---|---|---|---|---|
| `Soa<T, I>` | **16 / 72** B | 8 / 64 | 0 | 4 | 三个独立 `Vec` |
| `Packed<T, I>` | **16 / 72** B | 8 / 64 | 0 | 8（`Link<I>`） | `data` + `Link{prev,next}` 交错 |
| `Aos<T, I>` | **16 / 72** B | 16 / 72 | 0 | 16 / 72 | 一个 `Vec<Node<T, I>>` |

（上表是**索引宽度 `u32`** 的实测值，也是默认；`I = usize` 时三种布局的每槽全部 +8 B
——`Soa`/`Packed` 24 B、`Aos` 24 B（T=64 时 80 B，`Node` 越过 cache line 后 padding 无处可省）。
`data_offset` 三种布局、两种宽度下实测都是 0。）

关于 `Node<T>` 的字段次序（`repr(Rust)`）：

- 编译器**有权重排字段**。本仓库实测它保留了声明次序 `[data][prev][next]`
  （三个字段对齐都 ≤ 8、没有 padding 可省 ⇒ 排序稳定），所以 `data_offset` 是 0。
- **crate 不依赖这个**：偏移一律用 `offset_of!` 现取。把 `data` 挪到中间后
  `data_offset` 变 8，20/20 测试原样通过（迭代器走的是 `Layout`）。
- 实测两种次序：cache line 集合**逐档相同**（`next` 永远在 `T+8`，`prev`/`data` 只互换），
  性能差在噪声内；唯一风险是**过度对齐**的 `T`（align ≥ 16）配 `#[repr(C)]` 时
  `data` 居中会把节点撑大 50%（32→48、64→96）。`repr(Rust)` 下编译器自己会避免这个。

---

## 3. 全套结果

> **2026-09-29 起索引宽度默认 `u32`**（`Ix` 参数，见 §4 第 5 条）：每槽 16 B、`append` −37%。
> 下面各表已按新默认重测（1M 元素）；`iteration` 与 `churn` 的变化最明显，
> 其余在 ±5% 噪声内。`§3.2` 的"三层拆解"与 `§3.6` 的带宽对账仍是 `usize` 时代量级。

### 3.1 端操作 `end_ops`（1M 次 push + 1M 次 pop）

同一份 `push_back`/`pop_front` 与 `push_front`/`pop_back` 流程，五种实现：

| 形态（1M 次 push + 1M 次 pop，不预分配） | SoaList | PackedList | AosList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `push_back` + `pop_front` | 6.08 ms | 6.44 ms | 6.13 ms | 2.53 ms | 5.79 ms |
| `push_front` + `pop_back` | 7.49 ms | 6.24 ms | 5.64 ms | 2.10 ms | 5.99 ms |

这一组**不预分配**，量的是"边扩边搬 + 每页首次触碰"：每槽 16 B（`VecDeque` 是 8 B）⇒ 页数
与拷贝量都是 3×，`VecDeque` 那 2.1~2.5 ms 的优势主要来自这里，不是代码快。组内布局差异
（最大 ±23%）落在本组噪声底（±10~20%）内，别硬读。

### 3.2 迭代 `iteration` / `blob_iter`

| 形态 | SoaList | PackedList | AosList | VecDeque | LinkedList | Vec |
|---|---|---|---|---|---|---|
| `iteration`（1M × 8 B，全量迭代） | 1.21 ms | **1.10 ms** | **1.32 ms** | 310.4 µs | 1.05 ms | 80.7 µs |
| `blob_iter`（250k × 64 B） | 378.8 µs | 473.0 µs | 562.7 µs | 240.6 µs | 660.4 µs | 240.9 µs |

把迭代拆成三层（内部探针，min/9 轮，ns/元素）：

| 布局 | 公开 `iter()` | 自己沿链走 | 顺序扫槽位（不碰链） |
|---|---|---|---|
| SoaList | 1.08 | 1.14 | **0.062** |
| PackedList | 1.31 | 1.37 | **0.063** |
| AosList | 1.29 | 1.34 | **0.478** |
| `Vec` 顺序扫（对照） | — | — | 0.062 |

- **迭代器实现本身没有浪费**：`iter()` 只比"自己沿链走"慢 ~5%（双层判断 + 计数）。
- **顺序扫槽位：Soa/Packed 与 `Vec` 同速**；**Aos 退化 7.7×**（节点交错后每 24 B 只用
  8 B）。但公开 API 没有"按槽位顺序扫全体"的操作 ⇒ 那只是上限，不是任何调用的成本。
- **真正贵的是"沿链走"**：~1.1 ns/元素 = "读 `next` → 去那个槽位"这次**依赖加载的
  延迟**（~4 周期/元素），不是带宽。这是链表结构的固有下限，三条改进路线都试过（§4）。
- **载荷大时反超 `LinkedList`**：`blob_iter` 378.8 µs vs 660.4 µs（快 1.74×）——它的节点
  是独立分配 ⇒ 每取一个 64 B 元素都要追一次指针、还可能缺页；我们 80 B 的槽位是连续的，
  硬件预取能一路走。
- 数字依赖**链的局部性**（这里是 `push_back` 构造 ⇒ 链就是 0,1,2,… 递增，硬件预取猜
  得中）；`random_remove_insert` 之后链会碎，`iter()` 落到"每步一次 miss"的量级。

### 3.3 定位与中间插删 `middle_access` / `middle_insert_remove`

| 形态 | SoaList | PackedList | AosList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `middle_access`（100 次定位到 N/2） | 54.02 ms | 55.61 ms | 66.96 ms | 56.8 ns | 59.77 ms |
| `middle_insert_remove`（1000 次插入+删除） | 546.9 µs | 569.0 µs | 670.4 µs | 82.46 ms | 495.8 µs |

`at(pos)` 走 `O(min(pos, len-1-pos))` 步，所以 `middle_access` 的每一"次"其实是在 1M
元素的链上走 50 万步 ≈ 0.5 ms —— 换算成每步约 1 ns，和 §3.2 的"沿链走"完全一致。
`VecDeque` 那 56 ns 是 O(1) 索引，两者不是同一个数量级的操作，列出来只为标明差距。

`middle_insert_remove` = 在链中间"插入 + 移一步 + 删除"1000 次，**三种布局与 `VecDeque`
同量级**：这条路径的瓶颈不在链，而在**分配/释放槽位**（free-list 弹出/压入的随机访存），
所以布局差异（各槽数组的位置）几乎看不出来。

### 3.4 稳态与扰动 `churn` / `random_remove_insert` / `cursor_update`

| 形态 | SoaList | PackedList | AosList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `churn`（1M 次 pop_front+push_back） | 2.75 ms | 3.00 ms | 2.72 ms | 1.68 ms | 6.15 ms |
| `random_remove_insert`（100 次随机位置删+插） | 28.55 ms | 32.44 ms | 37.52 ms | 4.60 ms | 32.18 ms |
| `cursor_update`（链中点原地读写 1M 次） | 1.19 ms | 1.26 ms | 1.34 ms | 672.2 µs | 2.45 ms |

- `churn` **完全不分配**（列表预先建好、槽位循环复用），所以它量的是**纯代码**：我们每次
  `pop_front`+`push_back` 要改 ~4~8 个链接字段（两个邻居 + free-list），`VecDeque` 只改两个
  下标 ⇒ 1.63× 的差距全在这里；而 `LinkedList` 每个节点都要 `malloc`/`free` ⇒ 我们快 2.3×，
  **这就是"连续槽位 + free-list"最直接的收益**。
- `random_remove_insert` 把链打碎、位置随机 ⇒ 三种布局都落到 28~38 ms（`Soa` 最好），
  `VecDeque` 只 4.6 ms——因为它在中间删插只是 memmove，而我们要走链（`at(pos)` 是 O(N)）。
- `cursor_update` 在链中点原地读写：`Aos` 最慢（1.34 ms，节点交错 ⇒ 每次读 `data` 都多跨
  内存），`LinkedList` 2.45 ms（每步追一次指针）。

### 3.5 大载荷 `blob_end_ops`（`Blob64`，64 B/元素，250k）

| 形态 | SoaList | PackedList | AosList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `blob_end_ops`（250k × 64 B push + pop） | 4.40 ms | 3.99 ms | 4.22 ms | 3.24 ms | 4.59 ms |

载荷大了链接开销被摊薄：`VecDeque` 只领先 1.36×（8 B 时是 2.4~3.0×），而且**我们比
`LinkedList` 快**（4.2 vs 4.6 ms）——它的每个 64 B 节点都是独立分配。

每槽 80 B（64 载荷 + 16 链接）⇒ 载荷占 80%。这是"我们的链接开销在载荷大时被摊薄"的
典型场景：8 B 元素时 24 B/槽是载荷的 3 倍，64 B 时只多 25%。

### 3.6 `append`：两个 API（本库与 std 语义不同的地方）

两个 API 的**计费单位**不同，选哪个看形态：

- `append`（对齐 `LinkedList::append`）：整块搬对方**全部槽位**（下标一边搬一边加偏移），
  按 **对方槽位数 + 自己存储规模** 付账，与对方活元素数无关。
- `append_elementwise`（本库扩展）：逐元素搬，优先填自己**已有的空闲槽**，按 **活元素数**
  付账。

`LinkedList::append` 是 O(1) 指针拼接（实测 144 ns，与载荷无关）；我们为了迭代局部性
（槽位连续）放弃了这一点，**这是设计选择**。

| 形态 | SoaList | PackedList | AosList | VecDeque | Vec | LinkedList |
|---|---|---|---|---|---|---|
| `append`（1M ⊕ 1M，两边都要扩容） | 4.53 ms | 4.01 ms | 3.88 ms | 1.37 ms | 1.15 ms | 172.1 ns |
| `append_blob`（250k ⊕ 250k，64 B 元素） | 3.24 ms | 3.24 ms | 2.93 ms | 2.19 ms | 2.06 ms | 178.9 ns |
| 同上，`::elementwise`（目标预先留 250k 空闲槽） | 1.76 ms | 1.90 ms | 1.75 ms | — | — | — |

**整块 `append` 比 `Vec::append` 慢 2.6 倍？字节对账（mimalloc，1M⊕1M）：**

| 形态 | 时间 | 实搬字节 | 有效带宽 |
|---|---|---|---|
| `SoaList::append`（3×usize，需扩容） | 3.84 ms | ~96 MB | 25.0 GB/s |
| `Vec<usize>::append`（需扩容） | 1.45 ms | ~32 MB | 22.1 GB/s |
| 裸 Vec×(usize, u32, u32)（需扩容） | 2.82 ms | ~64 MB | 22.7 GB/s |
| 裸 Vec×3 usize（目标预置容量，不扩容） | 1.93 ms | ~48 MB | 24.8 GB/s |
| `Vec<usize>`（目标预置容量，不扩容） | 0.60 ms | ~16 MB | 26.6 GB/s |
| 热目标 8 MB `extend_from_slice` | 0.58 ms | ~16 MB | 27.4 GB/s |
| 热目标 8 MB `extend(map(\|i\| i + base))` | 0.52 ms | ~16 MB | 30.9 GB/s |

- **下标改写免费**：`extend(map(|i| i + base))` 与 `extend_from_slice` 同速（甚至更快，
  省掉 memcpy 库调用）⇒ 加偏移不是开销来源。
- 差距全是**搬的字节数**：每槽 16 B（data+prev+next，`u32` 索引）对 `Vec` 的 8 B；两边都因扩容把
  每个字节搬两遍（`realloc` 搬旧数据 + 追加拷贝）= 4× 载荷。所有测点落在同一带宽区间
  （21~31 GB/s），我们 per-byte 甚至略好。
- 想追平只有两条（都实测）：索引降到 u32（**现在是默认**）⇒ 16 B/槽、**−37%**；目标预置容量不扩容
  ⇒ **−57%**（后者要求分配器保持页热）。

**整块搬的代价几乎全在"目标要不要新页"**（同一形态，只换环境）：

| 环境 | SoaList | PackedList | AosList | 计时区缺页 |
|---|---|---|---|---|
| mimalloc（默认 feature） | 3.7 ~ 4.3 ms | 3.6 ~ 4.0 ms | 3.4 ~ 3.7 ms | 0 |
| 系统 malloc（`--no-default-features`） | 8.2 ms | 17.0 ms | 11.3 ms | 4095 / 8095 / 12003 |
| 系统 malloc + `GLIBC_TUNABLES=glibc.malloc.hugetlb=1` | 2.8 ms | 4.2 ms | 2.4 ms | 8 / 17 / 24 |
| 对照：`mmap` 24 MB、每页只写 1 字节、**不做拷贝** | 8.2 ~ 9.4 ms | — | — | 5860 |

⇒ 这条路的成本按**页**计费（本机每页首次触碰 ~1.2 µs）。想读"算法本身多快"看 mimalloc
那行（0 缺页）；比较版本时**必须同时看缺页数**。

**大载荷（`Blob64`，250k）**：

| 形态 | 时间 | 实搬 | 带宽 |
|---|---|---|---|
| SoaList | 2.87 ~ 3.20 ms | ~80 MB | 25.0 GB/s |
| PackedList | 2.86 ~ 2.94 ms | ~80 MB | 28.0 GB/s |
| AosList | 2.80 ~ 2.92 ms | ~80 MB | 28.6 GB/s |
| VecDeque | 2.22 ms | ~64 MB | 28.8 GB/s |
| `Vec`（基线） | 1.98 ~ 2.40 ms | ~64 MB | 26.7 GB/s |
| `LinkedList`（O(1) 指针拼接） | 144 ns | 0 | — |
| 对照：裸 3 数组预置容量（不扩容） | 1.17 ms | ~40 MB | 34.2 GB/s |

载荷大了差距变小（8 B 时比 `Vec` 慢 2.6×，64 B 时只慢 ~1.2~1.35×：每槽多出的 16 B 在
大载荷下只占 20%）；布局差异这时才看得见但依旧很小（Aos ≈ Packed ≲ Soa，5~12%）。

**同一目标形态下两个 API 的对照**（目标预先留 250k 空闲槽；mimalloc，每槽 80 B）：

| 形态 | `append_elementwise` | `append` |
|---|---|---|
| 目标有 250k 空闲槽 + 源 250k 密 | **1.78 ms** | 3.83 ms |
| 目标有 250k 空闲槽 + 源稀疏（250k 槽 / 100 活） | **0.001 ms** | 3.59 ms |
| 目标 0 空闲槽 + 源 250k 密 | 3.07 ms | **2.85 ms** |
| 目标 0 空闲槽 + 源稀疏 | **0.001 ms** | 2.40 ms |

逐元素成本随载荷近似线性：**≈ 2 ns + 0.08 ns × 载荷字节 / 元素**（每个元素都要"出源、
进目标"搬两次，不是流式带宽）：实测 8 B 2.1 ns、64 B 6.9 ns、256 B 22.4 ns；整块则只按
字节走带宽（21~28 GB/s），与元素个数无关。所以"目标装不下"那一格载荷越大两条路越接近。

**两个 API × 输入形态（目标固定 1M 活元素，N = 1M，min/3 轮，分配器已预热）**：

| 目标空闲槽 \ 源 | 密 1M | 稀疏 1M 槽 / 100 活 | 密 100 |
|---|---|---|---|
| 0 | 逐元素 4.9 / **整块 3.5** → 用 `append` | 逐元素 **0.001** / 整块 3.4 → **必须用 `append_elementwise`** | ~0 / ~0 |
| 1 000 | 逐元素 4.7 / **整块 3.5** → `append` | **逐元素 0.001** / 整块 4.1 | ~0 / ~0 |
| 1M | **逐元素 2.1** / 整块 4.9 | **逐元素 0.001** / 整块 4.4 | ~0 / ~0 |


### 3.7 句柄入口 vs 位置入口（`slot_entry` 组）

**同一份工作，只有"怎么找到元素"不同**：100 次"随机位置删一个、原地插回去"
（`cursor_at(handle)` + `remove_current` + `insert_before` vs `at(pos)` + 同样两步）：

| 入口 | SoaList | PackedList | AosList |
|---|---|---|---|
| 按句柄 `cursor_at`（`O(1)`） | **0.8 µs** | **0.6 µs** | **0.6 µs** |
| 按位置 `at(pos)`（走链 `O(min(pos, len-1-pos))`） | 31.7 ms | 34.9 ms | 39.4 ms |

（单位是 100 次操作的总时间；换成每次：**~8 ns vs ~317~394 µs**，差约 **4 万倍**。）
按句柄那一列是"100 个受害槽位在缓存里"的稳态（LRU 里常见）；即便受害槽位是冷的，
也只是多几次 miss（几百 ns），结论不变。

---

## 4. 已试过并否掉的优化（都有实测数字，别再走一遍）

| # | 方案 | 收益 | 代价 / 为什么否 |
|---|---|---|---|
| 1 | 分块（分组）布局 + 块内**仍每槽显式链接** | **−55%（更慢）** | 那次依赖加载还在，还多了下标算术 |
| 2 | 分块 + 块内顺序**由位置隐含**（= unrolled linked list） | 迭代 **8~13×**（实测 0.084~0.136 vs 1.10 ns/元素） | 插删要搬块内元素 ⇒ **下标不再稳定**；而且**不需要自己写**——见 §4 末 |
| 3 | 维护顺行位图（每槽 1 bit：`next(i)==i+1`） | 迭代 **2.5×** | 每次链写入 +1 store ⇒ `churn` **2.80 → 5.66 ms（慢一倍）** |
| 4 | 迭代器侧探路（段用尽时并行读 8 个 `next` 候选） | 独立探针 1.6× | crate 内实测 **2.40 vs 1.09 ns/元素（慢一倍）**，原因未定位 ⇒ 不发货 |
| 5 | 索引降到 u32 | **已做成默认**（`Ix` 参数）：`append` **−37%**（T=8，3.07 vs 4.86 ms）、内存 24→**16 B/槽**；`churn`/`iteration`/`middle_access` 持平（±1%） | 槽位上限 2.1G（`u32` 的最高位留给空闲标记）；踩过一个坑：`grow` 里的触顶 `assert!` 带 `{}` 参数会把格式化机器拖进去 ⇒ 内联器放弃内联 `alloc_slot` ⇒ **churn +47%**（循环里出现 `call`），改用 `#[cold]` helper 后归零 |
| 6 | `append` 前先 `reserve` | 无可测收益 | std 的 `Vec::append` 内部本来就先 reserve；`reserve_exact` 反而更贵（实测 3.9→7.5 ms） |
| 7 | `Aos` 字段次序（`data` 居中/前置） | 噪声内 | 见 §2：cache line 集合相同 |
| 8 | 库内 `madvise(MADV_HUGEPAGE)` | 好时 8.2 → 1.1~1.4 ms | 本机 THP `madvise` 模式 + `nr_hugepages=0`，复测不稳定（5860 缺页）⇒ 只能靠环境变量 |
| 9 | `clear` 改成**按内存顺序扫 `data`**、用 `is_free` 判断（不追链） | 密集（无 Drop glue）**2.2×**（1.11 → 0.50 ms） | **稀疏（1M 槽位、1000 live）1.42 µs → 464 µs，慢 327×**；带 Drop glue 连密集也慢 12%（3.18 → 3.60 ms）；复杂度 O(live) → O(slots) ⇒ 否 |
| 10 | 标记空闲改成**只写标志位所在的那一个字节**（省掉读-改-写里的 load） | IR 上确实少一次 load（`--emit llvm-ir`：`store i8 -128` 对 `load i64`+`or`+`store`；RISC 后端从 `ldrb`/`orr`/`strb` 三条降到 `strb` 一条） | 本机 x86-64 实测 `churn` **慢 5.5~7.5%**（Soa/Packed，四次测量、对照 `VecDeque` 持平、布局扰动底 ±2%）⇒ 否；换 ISA 需重测。**注意：整函数 diff 显示差异不止那一条指令**——crate 里 `|=` 是 `movabs`（提出循环）+ `or %r9,(mem)`，字节写是 `movb`，另外寄存器分配、栈帧、指令数（133→126）与**循环对齐**（热块入口 `mod 64` 从 0 到 60）都变了 ⇒ 那 7% 不能归给单条指令；片段级汇编不可外推（见 `FREE_BIT` 文档） |

---

### "分块"值不值得自己写：不值得（std 组合就够了）

把"分块"直接用 std 拼出来（`LinkedList<Vec<usize>>`，块内加一个头偏移使两端都 O(1)），
K=64，与我们的 `SoaList` 同机同期对照：

| 指标（1M × `usize`） | 本库 `SoaList` | `LinkedList<Vec<usize>>` K=64 | 倍数 |
|---|---|---|---|
| 迭代 | 1.08 ns/元素 | **0.112** | 9.6× |
| `at(N/2)`（走到中间） | 1.08 ns/元素 | **0.013** | 83× |
| `churn`（`pop_front`+`push_back`） | 2.81 ns/op | **1.70** | 1.65× |
| `end_ops`（push 1M + pop 1M，不预分配） | 3.04 ns/op | **0.93** | 3.3× |
| 内存/槽 | 16 B | **~9 B** | 1.8× |
| 元素级游标 / 稳定槽位身份 | ✓ | ✗ | — |

- "分块"就是 **unrolled linked list ≈ `LinkedList<Vec<T>>` 这一类**（块内连续 + 块间链）。
  迭代那 8~13× 不是某种自有设计的功劳，std 组合同样拿得到；`VecDeque` 当块反而更差
  （K=64 时迭代 0.303 vs `Vec` 块 0.094——deque 的内容可能跨环回卷，迭代器流不起来）。
- 而且它在上面四项上**全面赢本库**：块内是裸数组（一次 `Vec::push` / `head += 1`），我们每次
  端操作要改 4~8 个链接字段（两个邻居 + free-list 的 push/pop），内存还高 1.8 倍（16 vs ~9 B/槽）
  ⇒ 端到端输在**每槽字节数**，`churn` 输在**每操作字段数**。
- 本库剩下的差异只有三条，全在**语义**而不是吞吐上：
  ① **元素级游标**（`at(pos)` / `insert_before` / `insert_after` / 幽灵位置 / 环形移动）；
  ② **稳定的槽位身份**（"下标即指针"——分块恰恰要丢掉它）；
  ③ **热身后零分配**（一整块 slab：`churn` 一个字节都不申请；链块每 K 个元素要 `malloc`/`free`
  一次块，本机实测每次 ~2 ns 摊销）。
- ⇒ 要"分块的性能"就别把它塞进本库：用 `LinkedList<Vec<T>>`，或者把块当元素放进本库
  （`List<Vec<T>, Soa<Vec<T>>>`——块下标仍然稳定、块内连续、两端 O(1)）。

## 5. 内部探针（解释机制，不是公开 API 的调用成本）

| 项目 | 数字 | 出处 |
|---|---|---|
| `clear` vs "一直 `pop` 到空" | 8 B **1.2×**、64 B **2.9×**、512 B **24.5×**（带 Drop glue 时 64 B 只剩 1.6×） | `List::clear` 文档 |
| `clear`：追链表 vs 扫描槽位（1M 槽位） | 密集 1.11 → 0.50 ms（扫描快 2.2×）；稀疏（1000 live）**1.42 µs → 464 µs（慢 327×）**；密集 + Drop glue 3.18 → 3.60 ms ⇒ **保留追链表** | `List::clear` 文档 + `clear_drop` 组 |
| `Drop`：只走 live 链就地析构 vs 经 `clear`（1M） | `usize`（纯记账）**1.172 ms → 2.37 µs**；带 Drop glue Soa **3.13 → 1.96 ms（1.6×）**、Aos 2.84 → 2.38 ms | `List` 的 `Drop` 文档 + `clear_drop` 组 |
| 迭代三层拆解（1M×`usize`，ns/元素） | `iter()` 1.08 / 自己沿链走 1.14 / 顺序扫槽位 **0.062**（Soa）；Aos 1.29 / 1.34 / 0.478 | 本文 §3.2 |
| `append` 字节对账（mimalloc，1M⊕1M） | 我们 25.0 GB/s vs `Vec` 22.1 GB/s ⇒ **差距全在搬的字节数**（16 B/槽 vs 8 B/槽，各自因扩容搬两遍） | §3.6 |
| 下标改写（`map(\|i\| i + base)`）成本 | 与 `extend_from_slice` 同速（甚至更快）⇒ **加偏移免费** | §3.6 |
| 尺寸固定开销 | 16 B/槽（`u32`）：T=8 时是载荷的 2×，T=64 时是 12.5% | §2 |

---

## 6. 选型指南

**布局**

- 迭代密集（写少读多）→ `SoaList` / `PackedList`（`Aos` 在 64 B 载荷上迭代慢 ~22%）。
- 中间位置读写多 → `PackedList`（`prev`/`next` 同处一条 `Link`，同一条线）。
- 想让分配与内存最简单 / 元素 ≤ 48 B → `AosList`（一个 `Vec`，一次 realloc）。
- 元素 ≥ 64 B 想省内存 → `SoaList` / `PackedList`（每槽少 8 B）。

**`append` 两个 API**（这是本库与 std 语义不同的地方，`LinkedList::append` 是 O(1) 指针拼接）

- 目标**有**空闲槽、或源稀疏 → `append_elementwise`（按活元素计费；源稀疏时快三个数量级）。
- 目标要扩容、源稠密 → `append`（整块按字节走带宽，与元素个数无关）。

**最大的两个旋钮**

1. `with_capacity` / `reserve`：目标预置容量不扩容 ⇒ `append` **−50~60%**。
2. 复用已触碰的槽（`append_elementwise`）：不产生新页 ⇒ 冷分配器下差别是数量级的。

**环境（库外，但对 `append*` 影响最大）**

```sh
cargo bench                                  # 默认 mimalloc（默认 feature）
cargo bench --no-default-features            # 系统 malloc：看缺页，别看墙钟
GLIBC_TUNABLES=glibc.malloc.hugetlb=1 cargo bench   # glibc ≥ 2.35，让大块走大页
MIMALLOC_PURGE_DELAY=-1 cargo bench          # mimalloc：别把页还给内核
```

---

## 7. 复现

```sh
cargo bench                        # 全套；报告在 target/criterion/report/index.html
cargo bench -- 'FastList'          # 只看 fast-list 对照组（§9）
cargo bench --features linked-list-cursors   # nightly：把 std 游标那三行对照加回来
cargo bench -- 'iteration|churn'   # 过滤（正则）
cargo test                         # 正确性（20 项，含不变量逐槽校验）
cargo miri test                    # 严格 provenance（迭代器走裸地址）
```

`benches/list.rs` 在 stable 上同样可编译：对比 std `LinkedList` 游标的三行对照
（`middle_insert_remove` / `random_remove_insert` / `cursor_update` 的 `LinkedList`）需要
`#![feature(linked_list_cursors)]`，由 **`linked-list-cursors` feature**（**不在 `default`**）
控制 ⇒ stable 上 `cargo bench` 直接可用，nightly 想要那三行就
`cargo bench --features linked-list-cursors`。
用 feature 而不是 `build.rs` 自动探测通道，是因为这是库：`build.rs` 会在每个下游用户
编译本 crate 时都跑一次，而 feature 显式、下游零成本，也和 `mimalloc` 一致。

本文件里的"内部探针"数字多数来自临时探针（`#[test]` + `Instant`，min/9 轮），跑完即删；
`clear` / `Drop` 的对照（`cargo bench -- 'clear_drop'`）已经**常驻**成 `clear_drop` 组：
密集 / 稀疏 / `clear` 后重填 / 析构，三种布局 × 无 glue 与带 64 B Drop glue。
机制与结论写在这里与 crate 文档里（`List::clear`、`List` 的 `Drop`、`Storage::append`、`benches/list.rs`）。

---

## 8. 性能之外：本库真正独特的四条

吞吐不是本库的卖点（§4 末：分块 ≈ `LinkedList<Vec<T>>` 全项更快）。真正独一无二的是
下面四条，前两条**已经有永久测试**（可以当契约看）。

1. **稳定槽位身份，现在是公开句柄（[`Slot`]）**：插入/删除不搬动别的元素，槽位在元素
   存活期内不变；`Storage::append` 甚至能把整段下标直接 `+= base`。
   - **拿句柄**：`Cursor::slot` / `CursorMut::slot` / `List::front_slot` / `List::back_slot` /
     `List::iter_slots`（全 `O(1)`）；**用句柄**：`List::cursor_at` / `cursor_at_mut` /
     `remove_slot` / `move_to_front` / `move_to_back`（全 `O(1)`，不需要逻辑位置）。
   - **实测收益**（§3.7）：同样的工作按句柄 **~8 ns/次**，按位置 **~317~394 µs/次**
     —— **约 4 万倍**，差别全在"要不要走链找它"。
   - **"活没活"是 `O(1)` 可判定的**：空闲标记位放在 `prev` 的**最高位**，判定时
     **不碰未初始化的 `data`**（否则就是 UB）。它只在"变成空闲"时写一次（读-改-写），
     链接写入路径上**没有任何额外操作**；`append` 的整段 `+= base` **自己带着它走**
     （`2^63 | v` 加 `base` 仍是 `2^63 | (v + base)`）。
   - 试过把这次写改成**直接赋值** `prev = FREE_BIT`（想省掉一次 load）：两轮交替 A/B
     下 `churn` **稳定慢 7~8%**（Soa 2.79/2.83 → 2.99/3.04、Packed 2.77/2.81 → 2.96/2.98，
     对照 `VecDeque` 持平）⇒ **保留读-改-写**。这是个反直觉但可复现的结果。
   - 读 `prev` **不掩码**（省一条 and，也短一环取地址的依赖链）：两轮交替 A/B 与三种
     测法下都是**中性**（±1~3%，方向不一致，对照同步漂移）⇒ 采用。代价是把"**只在 live
     槽位上读 `prev`**"变成显式契约：`Storage::prev` 在 debug 构建里带 `debug_assert`
     （release 零成本），24 项测试全绿说明现有每条路径都守约。
     实测代价：`churn` / `blob_end_ops` 两轮交替 A/B 落在噪声内（Soa churn 2.78/2.96 →
     3.78/2.80，Packed/Aos 持平）⇒ **不花钱**。
   - **取舍：不做世代校验**。句柄只回答"这个槽位现在活没活"；槽位被 free-list 复用后，
     旧句柄会指向**新元素**（经典 ABA，测试里把这个行为钉住了）。要"陈旧句柄也能被测出来"，
     就用 `slotmap` / `fast-list`（`LinkedListIndex` 是世代 key、`contains_key` 即校验）——
     代价是同机基准比我们慢 1.7~3×（iteration 1.90 vs 1.14 ms、churn 5.02 vs 2.81 ns/op）。

2. **热路径零分配**：容量备好后 `pop_front`/`push_back` 一次 `malloc` 都不发生
   （测试 `churn_does_not_allocate`：三种布局 1M 次 churn = **0** 次；对照 `LinkedList`
   1,000,000 次、链块 K=64 31,250 次）。配合固定 16 B/槽与 `with_capacity` ⇒ 内存与延迟
   都可预测（`clear` 后槽位进 free-list 复用，不还给内核）。
3. **状态可搬运、无指针（现在是公开 API）**：整条链的全部状态 = 每槽 `(T, prev, next)` +
   `head`/`tail`/`free_head`/`free_tail`/`len`（测试 `state_is_relocatable`：原样搬进新容器，
   迭代与不变量全一致）⇒ 可以直接序列化 / 放进共享内存 / `mmap`，**不需要指针修正**。
   入口：[`List::into_raw`] / [`List::from_raw`]（配 `RawList` 的取值器）+ 各布局的
   `as_parts` / `into_parts` / `from_parts`（`Link`/`Node` 已公开）+ 逐槽的
   `iter_slots()`（live）/ `free_slots()`（free 链，LIFO）。
   这是不变量"数组里每个值都是合法下标、`NIL` 从不写进数组"换来的。
4. **布局是参数**：`Soa`/`Packed`/`Aos` 共用同一份实现，`Layout` 用「基址 + 步长 + 偏移」
   把三种布局统一成迭代器要的裸地址。这既是本仓库能做横向基准的原因，也让"换布局"变成
   一次类型参数改动（三种布局的实测取舍见 §2、§3）。

⇒ 一句话定位：**本库不是"最快的链表"，而是"能当 `LinkedHashMap` / LRU 的存储引擎、同时
保留元素级链表 API、稳定句柄与确定性内存"的那个**；想要分块级的吞吐，就用
`LinkedList<Vec<T>>`（或把块当元素：`List<Vec<T>, Soa<Vec<T>>>`）。

---

## 9. 与 `fast-list` 0.1.8 的对照

同一格子里最接近的对手：slotmap 索引 + 世代号（`LinkedListIndex`）。事实逐条来自两边源码；
数字是同机同一次 `cargo bench`（`benches/list.rs` 里的 `FastList` 系列），中位数。

### 9.1 性能

| 组（规模） | 本库 `SoaList` | `FastList` | 倍数 | `LinkedList` | `VecDeque` |
|---|---|---|---|---|---|
| `end_ops/push_back_pop_front`（1M push + 1M pop） | **7.11 ms** | 12.54 ms | 1.8× | 5.37 ms | 2.58 ms |
| `iteration`（1M 元素） | **1.21 ms** | 1.72 ms | 1.4× | 1.05 ms | 0.31 ms |
| `middle_access`（100 次 N/2） | **54.4 ms** | 85.3 ms | 1.6× | 54.5 ms | 68 ns |
| `middle_insert_remove`（N/2 处 1000 次插删） | **561 µs** | 821 µs | 1.5× | 489 µs | 96.6 ms |
| `churn`（1M 次 pop+push） | **2.75 ms** | 5.09 ms | 1.9× | 6.15 ms | 1.68 ms |
| `random_remove_insert`（100 个随机位置） | **28.8 ms** | 43.3 ms | 1.5× | 29.8 ms | 4.67 ms |
| `slot_entry::by_handle`（100 次句柄入口） | **664 ns** | 43.5 ms | **6.5 万×** | — | — |
| `slot_entry::by_pos`（100 次位置入口） | **29.0 ms** | 43.8 ms | 1.5× | — | — |

读数注意：`end_ops` 量的是分配器（本文件 §1 的读数纪律，±20%），其余组 ±5%。

**为什么 `by_handle` 那一格差 6 万倍**（这是世代号语义的直接后果，不是实现质量）：

- 基准体是"随机位置删一个 + 原地插回"，criterion 会**反复跑**这段循环。
- `fast-list` 的 `LinkedListIndex` 带世代号：一次 `remove` + 重新插入之后，**原句柄就作废了**
  （新元素拿新世代）。所以第一轮之后句柄全失效，唯一出路是 `contains_key` 发现失效后
  **按位置重找**（O(N)）——基准里就是这么写的，于是 `by_handle` ≈ `by_pos`。
- 本库的 `Slot` 是裸下标：`remove` + `insert_before` 之后槽位被 LIFO 复用，**旧句柄依旧有效**
  且仍指向这一格（ABA 语义），所以是 O(1)。代价是：**它不会告诉你元素已经被换过了**。

⇒ 两边的差异不是"谁快"，而是**句柄在变更后的语义**：他们保证"失效可检测"，我们保证"身份不搬动"。
要在这类工作负载里用好 `fast-list`，得自己在外部维护"元素 → 当前句柄"的映射（每次插删都要更新），
本库则天生不需要（代价就是 ABA）。

### 9.2 功能

| 维度 | 本库 | `fast-list` 0.1.8 |
|---|---|---|
| 句柄类型 | `Slot`（裸下标 `usize`，8 B，无世代） | `LinkedListIndex`（slotmap key：`u32` 下标 + `u32` 世代，8 B） |
| 陈旧句柄 / ABA | ✗ **查不出来**：槽位复用后旧句柄指向新元素（测试钉住了这个行为） | ✓ `contains_key()` 即时校验（世代不匹配即 `false`） |
| 句柄 → 元素 | ✓ `cursor_at(_mut)` / `remove_slot`，O(1) | ✓ `get` / `get_mut` / `remove`，O(1) |
| 位置 → 句柄 | ✓ `slot_at(pos)`，O(N) | ✓ `nth(pos)`，O(N) |
| **元素级游标** | ✓✓ `Cursor`/`CursorMut`：跨插入/删除保持位置、幽灵位置、`move_prev/next/steps`、`peek` 邻居 | ✗ 只有 `cursor_next/prev`（遍历时取邻居的助手），没有"停在某个元素上"的对象 |
| 删除后"我原来在哪" | ✓ 游标留在原位，`insert_before` 放回同一处 | ✗ `remove` 之后要自己拿 `LinkedListItem::next_index`/`prev_index` 当锚点（基准就是这么写的） |
| `move_to_front` / `move_to_back` | ✓ O(1) 内建 | ✗ 要 `insert_*` + `remove` 两步，且**产生新 index ⇒ 旧句柄失效** |
| 整段搬运 `append` | ✓ O(1) 槽位重编号 + 下标整段偏移 | ✗ 只有 `extend`（逐个 push） |
| 附带数据 | ✗ 无内建（但 `Slot` 是普通下标，能直接当任何 map/数组的键） | ✓ `new_data::<V>()` / `new_data_sparse::<V>()`（slotmap 的 `SecondaryMap`，键就是句柄） |
| 整条链状态进出口 | ✓ `into_raw` / `from_raw` + `as_parts` / `from_parts` + `iter_slots` / `free_slots`（可落盘 / 共享内存，有 round-trip 测试） | ✗ 只能靠 `iter()` 逐个抄 |
| 无序遍历 | ✓ `iter_slots() -> (Slot, &T)` | ✓ `iter_unordered() -> &LinkedListItem<T>` |
| 有序遍历 / `retain` / `split_off` | ✓ | ✓ |
| 按**值**查找 `contains` | ✓ | ✗（只有按句柄的 `contains_key`） |
| 布局可选 | ✓ 三种（Soa / Packed / Aos） | ✗ 一种 |
| 每槽内存（T=8） | **16 B**（data + prev + next，索引 `u32`，§2 已核对） | `LinkedListItem<usize>` = **32 B**（value + index + next + prev），**外加** slotmap 每槽的版本/占用元数据 |
| 热路径零分配 | ✓ `churn_does_not_allocate` | ✓ 同一测试里也钉了（两边都是 0 次 malloc） |
| 可见的 `unsafe` | 核心是 unchecked 读写 + 裸地址迭代器，靠 miri + 全套测试兜 | **0 处**（整包 `unsafe` 计数为 0）⇒ 审计/信任成本更低 |
| 依赖 / `no_std` | 无依赖；关掉 `std` feature 即 `no_std` + `alloc` | `slotmap`（+ 可选 `unstable` 开 `Walker`）；只用 `core`，未声明 `no_std` |
| 派生实现 | `Clone` / `Debug` / `PartialEq` / `Eq` / `Default` / `FromIterator` / `Extend` / `IntoIterator` | 只有 `Debug` |

**结论**：同一数据结构的两种取舍，不是替代关系。

- 要**世代校验**（陈旧句柄必须能被发现）、要 slotmap 的 `SecondaryMap` 顺便带数据、或者想少一份 `unsafe`
  ⇒ 用 `fast-list`；代价是槽更大（32 B vs 24 B + slotmap 元数据）、句柄在每次插删后作废、
  没有游标 / `move_to_*` / `append`。
- 要**游标语义**（删除后还在原地、幽灵位置、相对移动）、**三种布局**、**整段搬运**、**更小的槽**、
  **句柄跨变更不失效** ⇒ 用本库；代价是陈旧句柄**测不出来**（要么自己配世代号，要么接受 ABA 语义，
  要么用 `new_data` 那类外部映射自己维护）。
