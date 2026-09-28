//! 内存布局策略：`data` / `prev` / `next` 如何落到内存里。
//!
//! 三种策略的**算法完全相同**，只有存储方式不同：
//!
//! - [`Soa`]：三个独立 `Vec`（array of structs 的反面，structure of arrays）
//! - [`Packed`]：`data` 一个 `Vec`，`prev`/`next` 打包成 [`Link`] 交错存放
//! - [`Aos`]：三者放进一个 [`Node`]（array of structs）

use std::mem::{MaybeUninit, offset_of, size_of};

/// 「没有指向任何节点」的哨兵值。
pub(crate) const NIL: usize = usize::MAX;

/// [`IterMut`](crate::IterMut) 需要的裸地址布局：
/// 元素 `i` 的数据地址是 `data + i * data_stride + data_offset`，
/// 其 `prev` / `next` 字段地址分别是 `prev + i * prev_stride` /
/// `next + i * next_stride`。
#[doc(hidden)]
pub struct Layout {
    #[doc(hidden)]
    pub data: *mut u8,
    #[doc(hidden)]
    pub data_stride: usize,
    #[doc(hidden)]
    pub data_offset: usize,
    #[doc(hidden)]
    pub prev: *const u8,
    #[doc(hidden)]
    pub prev_stride: usize,
    #[doc(hidden)]
    pub next: *const u8,
    #[doc(hidden)]
    pub next_stride: usize,
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
    fn len(&self) -> usize;

    /// 是否没有任何槽位。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 追加一个未初始化的槽位并返回其下标。
    fn grow(&mut self) -> usize;

    fn data(&self, index: usize) -> &MaybeUninit<T>;
    fn data_mut(&mut self, index: usize) -> &mut MaybeUninit<T>;
    fn prev(&self, index: usize) -> usize;
    fn next(&self, index: usize) -> usize;
    fn set_prev(&mut self, index: usize, value: usize);
    fn set_next(&mut self, index: usize, value: usize);

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
    fn len(&self) -> usize {
        self.data.len()
    }

    #[inline]
    fn grow(&mut self) -> usize {
        self.data.push(MaybeUninit::uninit());
        self.prev.push(NIL);
        self.next.push(NIL);

        self.data.len() - 1
    }

    #[inline]
    fn data(&self, index: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(index) }
    }

    #[inline]
    fn data_mut(&mut self, index: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(index) }
    }

    #[inline]
    fn prev(&self, index: usize) -> usize {
        unsafe { *self.prev.get_unchecked(index) }
    }

    #[inline]
    fn next(&self, index: usize) -> usize {
        unsafe { *self.next.get_unchecked(index) }
    }

    #[inline]
    fn set_prev(&mut self, index: usize, value: usize) {
        unsafe { *self.prev.get_unchecked_mut(index) = value };
    }

    #[inline]
    fn set_next(&mut self, index: usize, value: usize) {
        unsafe { *self.next.get_unchecked_mut(index) = value };
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
    fn len(&self) -> usize {
        self.links.len()
    }

    #[inline]
    fn grow(&mut self) -> usize {
        self.data.push(MaybeUninit::uninit());
        self.links.push(Link {
            prev: NIL,
            next: NIL,
        });

        self.links.len() - 1
    }

    #[inline]
    fn data(&self, index: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(index) }
    }

    #[inline]
    fn data_mut(&mut self, index: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(index) }
    }

    #[inline]
    fn prev(&self, index: usize) -> usize {
        unsafe { self.links.get_unchecked(index).prev }
    }

    #[inline]
    fn next(&self, index: usize) -> usize {
        unsafe { self.links.get_unchecked(index).next }
    }

    #[inline]
    fn set_prev(&mut self, index: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(index).prev = value };
    }

    #[inline]
    fn set_next(&mut self, index: usize, value: usize) {
        unsafe { self.links.get_unchecked_mut(index).next = value };
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
    fn len(&self) -> usize {
        self.nodes.len()
    }

    #[inline]
    fn grow(&mut self) -> usize {
        self.nodes.push(Node {
            data: MaybeUninit::uninit(),
            prev: NIL,
            next: NIL,
        });

        self.nodes.len() - 1
    }

    #[inline]
    fn data(&self, index: usize) -> &MaybeUninit<T> {
        unsafe { &self.nodes.get_unchecked(index).data }
    }

    #[inline]
    fn data_mut(&mut self, index: usize) -> &mut MaybeUninit<T> {
        unsafe { &mut self.nodes.get_unchecked_mut(index).data }
    }

    #[inline]
    fn prev(&self, index: usize) -> usize {
        unsafe { self.nodes.get_unchecked(index).prev }
    }

    #[inline]
    fn next(&self, index: usize) -> usize {
        unsafe { self.nodes.get_unchecked(index).next }
    }

    #[inline]
    fn set_prev(&mut self, index: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(index).prev = value };
    }

    #[inline]
    fn set_next(&mut self, index: usize, value: usize) {
        unsafe { self.nodes.get_unchecked_mut(index).next = value };
    }

    fn layout(&mut self) -> Layout {
        let nodes = self.nodes.as_ptr() as *const u8;

        Layout {
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
