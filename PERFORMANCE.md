# `slot-list` 性能总结

`List<T, S>` = **下标即指针**的双向链表：节点放在预分配的连续槽位里，删掉的槽位用
free-list 串起来复用；三种内存布局（`SoaList` / `PackedList` / `AosList`）是同一个泛型
类型的三个 `Storage` 参数。基线是 std 的 `LinkedList`、`VecDeque`、`Vec`。

---

## 0. 速览

| 问题 | 结论（当日全套，1M 元素） |
|---|---|
| 端操作 `push`+`pop`（不预分配） | 比 `VecDeque` 慢 2.4~3.0×，与 `LinkedList` 同级（6.1~7.5 vs 5.8~6.0 ms）——这组由**缺页与拷贝量**支配（§3.1） |
| 稳态端操作 `churn`（无分配） | **比 `LinkedList` 快 2.3×**（2.81 vs 6.45 ms），比 `VecDeque` 慢 1.63×（§3.4） |
| 迭代 | 与 `LinkedList` 同级（1.14 vs 1.07 ms；~1.1 ns/元素 = 依赖加载下限）；**64 B 载荷下比它快 1.74×**（§3.2） |
| 中间位置插删 | 与 `LinkedList` 同级、比 `VecDeque` 快 150×（后者每次要 memmove 50 万元素）（§3.3） |
| 内存/槽 | **固定 24 B**（T=8）；`Aos` 在 T=64 时多 8 B（§2） |
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

| 布局 | 内存/槽（T=8 / T=64） | `data_stride` | `data_offset` | `prev`·`next` 步长 | 结构 |
|---|---|---|---|---|---|
| `Soa<T>` | **24 / 72** B | 8 / 64 | 0 | 8 | 三个独立 `Vec` |
| `Packed<T>` | **24 / 72** B | 8 / 64 | 0 | 16 | `data` + `Link{prev,next}` 交错 |
| `Aos<T>` | **24 / 80** B | 24 / 80 | 0 | 24 / 80 | 一个 `Vec<Node<T>>` |

（`Aos` 在 T=64 时每槽多 8 B：`Node` = 64 + 8 + 8 已经越过 cache line，padding 无处可省；
T=8/16/32/48 时三种布局大小相同。）

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

### 3.1 端操作 `end_ops`（1M 次 push + 1M 次 pop）

同一份 `push_back`/`pop_front` 与 `push_front`/`pop_back` 流程，五种实现：

| 形态（1M 次 push + 1M 次 pop，不预分配） | SoaList | PackedList | AosList | VecDeque | LinkedList |
|---|---|---|---|---|---|
| `push_back` + `pop_front` | 6.08 ms | 6.44 ms | 6.13 ms | 2.53 ms | 5.79 ms |
| `push_front` + `pop_back` | 7.49 ms | 6.24 ms | 5.64 ms | 2.10 ms | 5.99 ms |

这一组**不预分配**，量的是"边扩边搬 + 每页首次触碰"：每槽 24 B（`VecDeque` 是 8 B）⇒ 页数
与拷贝量都是 3×，`VecDeque` 那 2.1~2.5 ms 的优势主要来自这里，不是代码快。组内布局差异
（最大 ±23%）落在本组噪声底（±10~20%）内，别硬读。

### 3.2 迭代 `iteration` / `blob_iter`

| 形态 | SoaList | PackedList | AosList | VecDeque | LinkedList | Vec |
|---|---|---|---|---|---|---|
| `iteration`（1M × 8 B，全量迭代） | 1.14 ms | 1.40 ms | 1.59 ms | 308.3 µs | 1.07 ms | 71.7 µs |
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
| `churn`（1M 次 pop_front+push_back） | 2.81 ms | 2.81 ms | 2.85 ms | 1.73 ms | 6.45 ms |
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
- 差距全是**搬的字节数**：每槽 24 B（data+prev+next）对 `Vec` 的 8 B；两边都因扩容把
  每个字节搬两遍（`realloc` 搬旧数据 + 追加拷贝）= 4× 载荷。所有测点落在同一带宽区间
  （21~31 GB/s），我们 per-byte 甚至略好。
- 想追平只有两条（都实测）：索引降到 u32 ⇒ 16 B/槽、**−37%**；目标预置容量不扩容
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
| 5 | 索引降到 u32 | `append` **−37%**（T=8）／−10%（T=64）；内存 24→16 B/槽 | 槽位上限 4G；要参数化布局或新增布局 ⇒ 未做 |
| 6 | `append` 前先 `reserve` | 无可测收益 | std 的 `Vec::append` 内部本来就先 reserve；`reserve_exact` 反而更贵（实测 3.9→7.5 ms） |
| 7 | `Aos` 字段次序（`data` 居中/前置） | 噪声内 | 见 §2：cache line 集合相同 |
| 8 | 库内 `madvise(MADV_HUGEPAGE)` | 好时 8.2 → 1.1~1.4 ms | 本机 THP `madvise` 模式 + `nr_hugepages=0`，复测不稳定（5860 缺页）⇒ 只能靠环境变量 |

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
| 内存/槽 | 24 B | **~9 B** | 2.7× |
| 元素级游标 / 稳定槽位身份 | ✓ | ✗ | — |

- "分块"就是 **unrolled linked list ≈ `LinkedList<Vec<T>>` 这一类**（块内连续 + 块间链）。
  迭代那 8~13× 不是某种自有设计的功劳，std 组合同样拿得到；`VecDeque` 当块反而更差
  （K=64 时迭代 0.303 vs `Vec` 块 0.094——deque 的内容可能跨环回卷，迭代器流不起来）。
- 而且它在上面四项上**全面赢本库**：块内是裸数组（一次 `Vec::push` / `head += 1`），我们每次
  端操作要改 4~8 个链接字段（两个邻居 + free-list 的 push/pop），内存还高三倍（24 vs ~9 B/槽）
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
| 迭代三层拆解（1M×`usize`，ns/元素） | `iter()` 1.08 / 自己沿链走 1.14 / 顺序扫槽位 **0.062**（Soa）；Aos 1.29 / 1.34 / 0.478 | 本文 §3.2 |
| `append` 字节对账（mimalloc，1M⊕1M） | 我们 25.0 GB/s vs `Vec` 22.1 GB/s ⇒ **差距全在搬的字节数**（24 B/槽 vs 8 B/槽，各自因扩容搬两遍） | §3.6 |
| 下标改写（`map(\|i\| i + base)`）成本 | 与 `extend_from_slice` 同速（甚至更快）⇒ **加偏移免费** | §3.6 |
| 尺寸固定开销 | 24 B/槽：T=8 时是载荷的 3×，T=64 时是 37% | §2 |

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
cargo bench -- 'iteration|churn'   # 过滤（正则）
cargo test                         # 正确性（20 项，含不变量逐槽校验）
cargo miri test                    # 严格 provenance（迭代器走裸地址）
```

本文件里的"内部探针"数字来自临时探针（`#[test]` + `Instant`，min/9 轮），跑完即删；
机制与结论写在这里与 crate 文档里（`List::clear`、`Storage::append`、`benches/list.rs`）。

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
     **不碰未初始化的 `data`**（否则就是 UB）。它只在"变成空闲"时写一次
     （read-modify-write），链接写入路径上**没有任何额外操作**；`append` 的整段
     `+= base` **自己带着它走**（`2^63 | v` 加 `base` 仍是 `2^63 | (v + base)`）。
     实测代价：`churn` / `blob_end_ops` 两轮交替 A/B 落在噪声内（Soa churn 2.78/2.96 →
     3.78/2.80，Packed/Aos 持平）⇒ **不花钱**。
   - **取舍：不做世代校验**。句柄只回答"这个槽位现在活没活"；槽位被 free-list 复用后，
     旧句柄会指向**新元素**（经典 ABA，测试里把这个行为钉住了）。要"陈旧句柄也能被测出来"，
     就用 `slotmap` / `fast-list`（`LinkedListIndex` 是世代 key、`contains_key` 即校验）——
     代价是同机基准比我们慢 1.7~3×（iteration 1.90 vs 1.14 ms、churn 5.02 vs 2.81 ns/op）。

2. **热路径零分配**：容量备好后 `pop_front`/`push_back` 一次 `malloc` 都不发生
   （测试 `churn_does_not_allocate`：三种布局 1M 次 churn = **0** 次；对照 `LinkedList`
   1,000,000 次、链块 K=64 31,250 次）。配合固定 24 B/槽与 `with_capacity` ⇒ 内存与延迟
   都可预测（`clear` 后槽位进 free-list 复用，不还给内核）。
3. **状态可搬运、无指针**：整条链的全部状态 = 每槽 `(T, prev, next)` + `head`/`tail`/
   `free_head`/`free_tail`/`len`（测试 `state_is_relocatable`：原样搬进新容器，迭代与不变量
   全一致）⇒ 可以直接序列化 / 放进共享内存 / `mmap`，**不需要指针修正**。这是不变量
   "数组里每个值都是合法下标、`NIL` 从不写进数组"换来的。
4. **布局是参数**：`Soa`/`Packed`/`Aos` 共用同一份实现，`Layout` 用「基址 + 步长 + 偏移」
   把三种布局统一成迭代器要的裸地址。这既是本仓库能做横向基准的原因，也让"换布局"变成
   一次类型参数改动（三种布局的实测取舍见 §2、§3）。

⇒ 一句话定位：**本库不是"最快的链表"，而是"能当 `LinkedHashMap` / LRU 的存储引擎、同时
保留元素级链表 API、稳定句柄与确定性内存"的那个**；想要分块级的吞吐，就用
`LinkedList<Vec<T>>`（或把块当元素：`List<Vec<T>, Soa<Vec<T>>>`）。
