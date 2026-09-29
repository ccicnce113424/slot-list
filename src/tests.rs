//! 三种布局共用一套测试：实现是同一份，所以只需要泛型函数 ×3。

use super::*;
use crate::storage::{NIL, Storage};
use std::cell::Cell;
use std::rc::Rc;

/// 对三种布局各跑一遍 `$check`（`i32` 元素）。
macro_rules! each_layout {
    ($check:ident) => {
        $check::<Soa<i32>>();
        $check::<Packed<i32>>();
        $check::<Aos<i32>>();
    };
}

// ============================================================
// 基本操作
// ============================================================

fn check_basics<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::default();

    assert_eq!(list.len(), 0);
    assert!(list.is_empty());
    assert_eq!(list.front(), None);
    assert_eq!(list.back(), None);
    assert_eq!(list.pop_front(), None);
    assert_eq!(list.pop_back(), None);

    list.push_back(1);
    list.push_back(3);
    list.push_front(0);

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![0, 1, 3]);
    assert_eq!(list.front().copied(), Some(0));
    assert_eq!(list.back().copied(), Some(3));

    *list.front_mut().unwrap() = 10;
    *list.back_mut().unwrap() = 30;

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![10, 1, 30]);

    assert!(list.contains(&1));
    assert!(!list.contains(&99));

    assert_eq!(list.pop_front(), Some(10));
    assert_eq!(list.pop_back(), Some(30));
    assert_eq!(list.pop_back(), Some(1));
    assert_eq!(list.pop_back(), None);

    list.push_back(7);
    list.clear();

    assert!(list.is_empty());
    assert_eq!(list.iter().next(), None);
    assert_invariants(&list);
}

// ============================================================
// 迭代
// ============================================================

fn check_iteration<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..5).collect();

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        list.iter().rev().copied().collect::<Vec<_>>(),
        vec![4, 3, 2, 1, 0]
    );

    let mut iter = list.iter();

    assert_eq!(iter.size_hint(), (5, Some(5)));
    assert_eq!(iter.next(), Some(&0));
    assert_eq!(iter.next_back(), Some(&4));
    assert_eq!(iter.len(), 3);
    assert_eq!(iter.next(), Some(&1));
    assert_eq!(iter.next(), Some(&2));
    assert_eq!(iter.next(), Some(&3));
    assert_eq!(iter.next(), None);

    for value in list.iter_mut() {
        *value *= 2;
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 2, 4, 6, 8]
    );

    for value in &mut list {
        *value += 1;
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![1, 3, 5, 7, 9]
    );

    let mut back_iter = list.iter_mut();

    assert_eq!(back_iter.next_back().copied(), Some(9));
    assert_eq!(back_iter.next_back().copied(), Some(7));

    let owned: Vec<i32> = list.clone().into_iter().collect();

    assert_eq!(owned, vec![1, 3, 5, 7, 9]);

    let mut into = list.into_iter();

    assert_eq!(into.size_hint(), (5, Some(5)));
    assert_eq!(into.next(), Some(1));
    assert_eq!(into.next_back(), Some(9));
    assert_eq!(into.len(), 3);
    assert_eq!(into.next(), Some(3));
    assert_eq!(into.next(), Some(5));
    assert_eq!(into.next(), Some(7));
    assert_eq!(into.next(), None);

    let empty: List<i32, S> = List::default();

    assert_eq!(empty.iter().count(), 0);
    assert_eq!(empty.into_iter().next(), None);
}

// ============================================================
// 游标：std 语义
// ============================================================

fn check_cursor_read<S: Storage<i32> + Default>() {
    let list: List<i32, S> = (0..3).collect();

    let mut cursor = list.cursor_front();

    assert_eq!(cursor.index(), Some(0));
    assert_eq!(cursor.current().copied(), Some(0));
    assert_eq!(cursor.peek_prev().copied(), None);
    assert_eq!(cursor.peek_next().copied(), Some(1));
    assert_eq!(cursor.front().copied(), Some(0));
    assert_eq!(cursor.back().copied(), Some(2));
    assert_eq!(cursor.as_list().len(), 3);

    cursor.move_next();
    assert_eq!(cursor.index(), Some(1));
    assert_eq!(cursor.current().copied(), Some(1));

    cursor.move_prev();
    assert_eq!(cursor.index(), Some(0));

    // 从首元素 move_prev 进入幽灵位置。
    cursor.move_prev();
    assert_eq!(cursor.index(), None);
    assert_eq!(cursor.current(), None);
    assert_eq!(cursor.peek_next().copied(), Some(0));
    assert_eq!(cursor.peek_prev().copied(), Some(2));

    // 幽灵位置 move_next 回到首元素。
    cursor.move_next();
    assert_eq!(cursor.index(), Some(0));

    // 从尾元素 move_next 进入幽灵位置。
    let mut cursor = list.cursor_back();

    assert_eq!(cursor.index(), Some(2));
    cursor.move_next();
    assert_eq!(cursor.index(), None);
    cursor.move_prev();
    assert_eq!(cursor.index(), Some(2));

    let empty: List<i32, S> = List::default();
    let mut cursor = empty.cursor_front();

    assert_eq!(cursor.index(), None);
    assert_eq!(cursor.current(), None);
    assert_eq!(cursor.peek_next(), None);
    cursor.move_next();
    assert_eq!(cursor.index(), None);
    cursor.move_prev();
    assert_eq!(cursor.index(), None);
}

fn check_cursor_write<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..3).collect();

    {
        let mut cursor = list.cursor_front_mut();

        assert_eq!(cursor.index(), Some(0));

        *cursor.current().unwrap() = 10;
        *cursor.peek_next().unwrap() = 20;

        assert_eq!(cursor.as_cursor().current().copied(), Some(10));
        assert_eq!(cursor.as_list().len(), 3);
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![10, 20, 2]);

    // insert_before / insert_after 后游标仍指向原节点。
    {
        let mut cursor = list.cursor_front_mut();

        cursor.move_next(); // -> 20
        assert_eq!(cursor.index(), Some(1));

        cursor.insert_before(15);
        assert_eq!(cursor.index(), Some(2));
        assert_eq!(*cursor.current().unwrap(), 20);

        cursor.insert_after(25);
        assert_eq!(cursor.index(), Some(2));
        assert_eq!(*cursor.current().unwrap(), 20);
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![10, 15, 20, 25, 2]
    );

    // remove_current：游标移到下一个元素。
    {
        let mut cursor = list.cursor_front_mut();

        cursor.move_next(); // -> 15
        assert_eq!(cursor.remove_current(), Some(15));
        assert_eq!(cursor.index(), Some(1));
        assert_eq!(*cursor.current().unwrap(), 20);
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![10, 20, 25, 2]
    );

    // 删尾元素 -> 幽灵位置。
    {
        let back = list.len() - 1;
        let mut cursor = list.at(back).unwrap();

        assert_eq!(cursor.remove_current(), Some(2));
        assert_eq!(cursor.index(), None);
        assert_eq!(cursor.remove_current(), None);
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![10, 20, 25]);

    // 幽灵位置的 insert_before 追加到末尾，insert_after 插到最前。
    {
        let mut cursor = list.cursor_front_mut();

        cursor.move_prev(); // -> ghost
        assert_eq!(cursor.index(), None);

        cursor.insert_before(99);
        assert_eq!(cursor.index(), None);
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![10, 20, 25, 99]
    );

    {
        let mut cursor = list.cursor_front_mut();

        cursor.move_prev(); // -> ghost
        cursor.insert_after(1);
        assert_eq!(cursor.index(), None);
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![1, 10, 20, 25, 99]
    );
}

fn check_cursor_push_pop<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..3).collect();

    {
        let mut cursor = list.cursor_front_mut();

        cursor.move_next(); // 指向 1，pos 1

        cursor.push_front(-1);
        assert_eq!(cursor.index(), Some(2));
        assert_eq!(*cursor.current().unwrap(), 1);

        cursor.push_back(-2);
        assert_eq!(cursor.index(), Some(2));

        assert_eq!(cursor.pop_back(), Some(-2));
        assert_eq!(cursor.index(), Some(2));

        assert_eq!(cursor.pop_front(), Some(-1));
        assert_eq!(cursor.index(), Some(1));
        assert_eq!(*cursor.current().unwrap(), 1);

        assert_eq!(cursor.front().copied(), Some(0));
        assert_eq!(cursor.back().copied(), Some(2));
        assert_eq!(cursor.front_mut().copied(), Some(0));
        assert_eq!(cursor.back_mut().copied(), Some(2));
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2]);

    // pop_front 删掉游标所指元素时，游标移到新队首。
    {
        let mut cursor = list.cursor_front_mut();

        assert_eq!(cursor.pop_front(), Some(0));
        assert_eq!(cursor.index(), Some(0));
        assert_eq!(*cursor.current().unwrap(), 1);
    }

    // pop_back 删掉游标所指元素时，游标移到幽灵位置。
    {
        let mut cursor = list.cursor_back_mut();

        assert_eq!(cursor.pop_back(), Some(2));
        assert_eq!(cursor.index(), None);
    }
}

fn check_cursor_extras<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..10).collect();

    {
        let mut cursor = list.at(7).unwrap();

        assert_eq!(cursor.index(), Some(7));
        assert_eq!(*cursor.current().unwrap(), 7);

        assert!(cursor.seek(2));
        assert_eq!(*cursor.current().unwrap(), 2);
        assert!(!cursor.seek(10));
        assert_eq!(*cursor.current().unwrap(), 2);

        assert!(cursor.move_steps(3));
        assert_eq!(*cursor.current().unwrap(), 5);
        assert!(!cursor.move_steps(-100));
        assert_eq!(*cursor.current().unwrap(), 5);

        assert!(!cursor.is_head());
        assert!(!cursor.is_tail());

        assert!(cursor.seek(0));
        assert!(cursor.is_head());

        assert!(cursor.seek(9));
        assert!(cursor.is_tail());
    }

    assert!(list.at(0).is_some());
    assert!(list.at(9).is_some());
    assert!(list.at(10).is_none());

    let empty: List<i32, S> = List::default();

    assert!(empty.clone().at(0).is_none());
}

// ============================================================
// 批量操作与 trait
// ============================================================

fn check_bulk<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..10).collect();

    list.retain(|value| *value % 2 == 0);

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 2, 4, 6, 8]
    );

    let mut other: List<i32, S> = (100..103).collect();

    list.append(&mut other);

    assert!(other.is_empty());
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 2, 4, 6, 8, 100, 101, 102]
    );
    assert_invariants(&list);
    assert_invariants(&other);

    let tail = list.split_off(5);

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 2, 4, 6, 8]
    );
    assert_eq!(
        tail.iter().copied().collect::<Vec<_>>(),
        vec![100, 101, 102]
    );

    assert_eq!(list.remove(0), 0);
    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![2, 4, 6, 8]);

    let empty: List<i32, S> = List::default();
    let mut empty2 = empty;

    assert_eq!(empty2.split_off(0).len(), 0);
}

// ============================================================
// 不变量
// ============================================================

/// 检查 `List` 文档里列出的全部不变量（只读，不改状态）。
/// 槽位是否在 free 链上（测试用）。按**个数**走（free 链的尾巴是自环，
/// `NIL` 从不进数组，所以不能"走到 NIL 为止"）。
fn free_set<S: Storage<T>, T>(list: &List<T, S>) -> Vec<bool> {
    let mut on_free = vec![false; list.storage.slots()];
    let free = list.storage.slots() - list.len();
    let mut slot = list.free_head;

    for step in 0..free {
        assert!(!on_free[slot], "free 链有环（第 {step} 步）");
        on_free[slot] = true;
        slot = list.storage.next(slot);
    }

    on_free
}

fn assert_invariants<T, S: Storage<T>>(list: &List<T, S>) {
    let slots = list.storage.slots();
    let len = list.len();

    // 1) `next` 里每个值都是合法下标（**没有哨兵值**：`append` 能把整段下标直接
    //    `+= base`，靠的就是这条）；`prev` 只对 **live** 槽位要求合法——空闲槽的
    //    `prev` 是陈旧值（唯一用途是那个空闲标记位），不参与任何运算。
    for i in 0..slots {
        assert!(list.storage.next(i) < slots, "next[{i}] 不是合法下标");

        if !list.storage.is_free(i) {
            assert!(list.storage.prev(i) < slots, "prev[{i}] 不是合法下标");
        }
    }

    // 2) 空表时两个端点字段归位
    assert_eq!(list.head == NIL, len == 0, "head 与 len 不一致");
    assert_eq!(list.tail == NIL, len == 0, "tail 与 len 不一致");

    let mut seen = vec![false; slots];
    let mut node = list.head;
    let mut last = NIL;

    // 3) live 链：从 head 走 len 步恰好覆盖所有 live 节点，并停在 tail
    for step in 0..len {
        assert!(node != NIL, "live 链比 len 短");
        assert!(!seen[node], "live 链有重复节点（第 {step} 步）");
        seen[node] = true;

        let prev = list.storage.prev(node);
        let next = list.storage.next(node);

        // 两端的"哑元"字段只在有真实邻居时才要求互逆
        if node != list.head {
            assert_eq!(list.storage.next(prev), node, "next(prev(node)) != node");
        }

        if node != list.tail {
            assert_eq!(list.storage.prev(next), node, "prev(next(node)) != node");
        }

        last = node;
        node = next;
    }

    if len > 0 {
        assert_eq!(last, list.tail, "走完 len 步没停在 tail");
    }

    // 4) free 链：长度恰好是 `slots - len`，与 live 链不相交，终点是 free_tail
    let free = slots - len;

    assert_eq!(list.free_head == NIL, free == 0, "free_head 与空闲数不一致");
    assert_eq!(list.free_tail == NIL, free == 0, "free_tail 与空闲数不一致");

    let mut node = list.free_head;
    let mut last = NIL;

    for step in 0..free {
        assert!(node != NIL, "free 链比空闲数短");
        assert!(!seen[node], "槽位同时在 live 链和 free 链（第 {step} 步）");
        seen[node] = true;

        last = node;
        node = list.storage.next(node);
    }

    if free > 0 {
        assert_eq!(last, list.free_tail, "free 链没走到 free_tail");
    }

    assert!(seen.iter().all(|&s| s), "有槽位不在任何链上");

    // 5) 空闲标记位必须与"在不在 free 链上"完全一致（句柄的存活判定全靠它）：
    //    free 链上的槽位 → `is_free` 为真；live 链上的 → 为假。
    let on_free_chain = free_set(list);

    for (i, &on_free) in on_free_chain.iter().enumerate() {
        assert_eq!(
            list.storage.is_free(i),
            on_free,
            "槽位 {i} 的空闲标记位与 free 链不一致"
        );
    }
}

/// 一长串操作，每步之后都验一遍不变量。
fn check_invariants_seq<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::default();
    assert_invariants(&list);

    // 空表 → 单节点 → 多节点
    list.push_back(0);
    assert_invariants(&list);
    list.push_back(1);
    list.push_front(-1);
    assert_invariants(&list);

    for i in 2..8 {
        list.push_back(i);
        assert_invariants(&list);
    }

    // 端点删除（含删到只剩一个）
    for _ in 0..3 {
        list.pop_front();
        assert_invariants(&list);
    }

    for _ in 0..2 {
        list.pop_back();
        assert_invariants(&list);
    }

    // 中间删除 + free 链复用
    list.remove(0);
    assert_invariants(&list);
    list.remove(list.len() - 1);
    assert_invariants(&list);

    // retain（可能删掉链头、链尾，也可能删空）
    list.retain(|value| *value % 2 == 0);
    assert_invariants(&list);

    // 游标插入 / 删除（含插在两端）
    {
        let mut cursor = list.cursor_front_mut();

        cursor.insert_before(70);
        assert_invariants(cursor.as_list());

        cursor.insert_after(71);
        assert_invariants(cursor.as_list());

        cursor.move_next();
        cursor.remove_current();
        assert_invariants(cursor.as_list());
    }

    // 在链头之前 / 链尾之后插入
    {
        let mut cursor = list.cursor_front_mut();

        cursor.insert_before(-100);
        assert_invariants(cursor.as_list());
    }

    {
        let mut cursor = list.cursor_back_mut();

        cursor.insert_after(100);
        assert_invariants(cursor.as_list());
    }

    // clear：所有槽位进 free 链，再重填到超过原有槽位数
    let slots = list.storage.slots();
    list.clear();
    assert_invariants(&list);
    assert_eq!(list.storage.slots(), slots);

    for i in 0..slots + 1 {
        list.push_back(i as i32);
        assert_invariants(&list);
    }

    // 删空：端点字段必须归位成 NIL
    while !list.is_empty() {
        list.pop_front();
        assert_invariants(&list);
    }

    // split_off
    let mut list: List<i32, S> = (0..6).collect();
    let tail = list.split_off(2);
    assert_invariants(&list);
    assert_invariants(&tail);

    // 用 retain 删空
    let mut list: List<i32, S> = (0..5).collect();
    list.retain(|_| false);
    assert_invariants(&list);
    assert!(list.is_empty());
}

// ============================================================
// append
// ============================================================

fn check_append<S: Storage<i32> + Default>() {
    // 空 ⊕ 空
    let mut a: List<i32, S> = List::default();
    let mut b: List<i32, S> = List::default();

    a.append(&mut b);
    assert!(a.is_empty() && b.is_empty());
    assert_invariants(&a);
    assert_invariants(&b);

    // 空 ⊕ 非空（`a` 一个槽位都没有：base == 0）
    let mut b: List<i32, S> = (0..5).collect();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    assert_eq!(a.len(), 5);
    assert!(b.is_empty());
    assert_invariants(&a);
    assert_invariants(&b);

    // 非空 ⊕ 空（对方一个槽位都没有：直接返回）
    let mut b: List<i32, S> = List::default();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    assert_invariants(&a);

    // 非空 ⊕ 非空：自己没有空闲槽、对方够密 ⇒ 走整块搬运，
    // 对方的空闲槽也要一起接过来
    let a_slots = a.storage.slots();
    let a_free = a.storage.slots() - a.len();
    let mut b: List<i32, S> = (10..20).collect();

    assert_eq!(b.pop_front(), Some(10));
    assert_eq!(b.pop_back(), Some(19));
    assert_eq!(b.storage.slots(), 10);

    let b_free = b.storage.slots() - b.len();

    assert_eq!(b_free, 2);
    assert_eq!(a_free, 0);

    a.append(&mut b);

    assert_eq!(a.len(), 13);
    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 11, 12, 13, 14, 15, 16, 17, 18]
    );
    assert_eq!(a.storage.slots(), a_slots + 10);
    assert!(b.is_empty());
    assert_eq!(b.storage.slots(), 0, "槽位搬走了，容量留下");
    assert_invariants(&a);
    assert_invariants(&b);

    // 搬过来的空闲槽必须能被复用（free 链拼接正确）
    let free = a.storage.slots() - a.len();
    let slots = a.storage.slots();

    assert_eq!(free, a_free + b_free, "空闲槽数不对");

    for i in 0..free {
        a.push_back(i as i32);
    }

    assert_eq!(a.storage.slots(), slots, "空闲槽没有全部复用");

    a.push_back(-1);
    assert!(a.storage.slots() > slots, "空闲槽用完之后应该扩容");
    assert_invariants(&a);

    // `self` 空但带空闲槽（clear 之后）：仍然走整块搬运
    let mut a: List<i32, S> = (0..3).collect();

    a.clear();

    let mut b: List<i32, S> = (0..4).collect();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    assert_eq!(a.len(), 4);
    assert_invariants(&a);

    // 连续追加多次，链越来越长（多段拼接之后下标仍然自洽）
    for round in 0..3 {
        let mut b: List<i32, S> = (0..4).collect();

        b.pop_front();
        a.append(&mut b);

        assert_eq!(a.len(), 4 + (round + 1) * 3);
        assert_invariants(&a);
    }

    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]
    );

    // 追加到空表：`base == 0`，但两条链都要接对
    let mut a: List<i32, S> = List::default();
    let mut b: List<i32, S> = (0..6).collect();

    b.pop_front();
    b.pop_back();
    a.append(&mut b);

    assert_eq!(a.storage.slots(), 6);
    assert_eq!(a.storage.slots() - a.len(), 2);
    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_invariants(&a);

    for i in 0..3 {
        a.push_front(i);
        assert_invariants(&a);
    }

    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![2, 1, 0, 1, 2, 3, 4]
    );

    // 自己接不下（空闲槽 0 < 对方 9 个活元素）⇒ 整块连接
    let mut a: List<i32, S> = (0..4).collect();
    let mut b: List<i32, S> = (300..310).collect();

    b.pop_front();

    let len_before = a.len();
    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    assert!(a.storage.slots() - a.len() < b.len());

    a.append(&mut b);

    assert_eq!(a.len(), len_before + 9);
    assert_eq!(a.storage.slots(), a_slots + b_slots);
    assert_eq!(b.storage.slots(), 0);
    assert_invariants(&a);
    assert_invariants(&b);

    // 对方很稀疏：整块连接照**槽位数**搬（36 个死槽也跟着过来）
    let mut a: List<i32, S> = (0..4).collect();
    let mut b: List<i32, S> = (0..40).collect();

    for _ in 0..36 {
        b.pop_front();
    }

    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    assert_eq!(
        (a.len(), a.storage.slots(), b.len(), b.storage.slots()),
        (4, 4, 4, 40)
    );

    a.append(&mut b);

    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 36, 37, 38, 39]
    );
    assert_eq!(
        a.storage.slots(),
        a_slots + b_slots,
        "整块连接会把对方的槽位（含死槽）一起接过来"
    );
    assert_eq!(b.storage.slots(), 0);
    assert_invariants(&a);
    assert_invariants(&b);

    // 对方是"空表但带槽位"：整块连接会把那些空槽也接过来
    let mut a: List<i32, S> = (0..4).collect();
    let mut b: List<i32, S> = (0..5).collect();

    b.clear();

    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    a.append(&mut b);

    assert_eq!(a.len(), 4);
    assert_eq!(a.storage.slots(), a_slots + b_slots, "空槽也被整块接过来");
    assert_eq!(b.storage.slots(), 0);
    assert_invariants(&a);
    assert_invariants(&b);
}

/// `append_elementwise`：逐元素搬，优先填自己已有的空闲槽（不扩容、不搬对方槽位）。
fn check_append_elementwise<S: Storage<i32> + Default>() {
    // a: 10 槽 / 4 活（6 空闲）；b: 3 槽 / 3 活 ⇒ 全部塞进空闲槽
    let mut a: List<i32, S> = (0..10).collect();

    for _ in 0..6 {
        a.pop_front();
    }

    let mut b: List<i32, S> = (200..203).collect();
    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    assert_eq!(a.storage.slots() - a.len(), 6);
    assert_eq!(b.len(), 3);

    a.append_elementwise(&mut b);

    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![6, 7, 8, 9, 200, 201, 202]
    );
    assert_eq!(a.storage.slots(), a_slots, "复用空闲槽，不该扩容");
    assert_eq!(a.storage.slots() - a.len(), 6 - 3);
    assert!(b.is_empty());
    assert_eq!(b.storage.slots(), b_slots, "对方的槽位留在对方那里");
    assert_invariants(&a);
    assert_invariants(&b);

    // 对方稀疏（1M 槽位只剩 100 活的情形在基准里量）：只搬活元素，不碰对方的死槽
    let mut a: List<i32, S> = (0..8).collect();

    for _ in 0..4 {
        a.pop_front();
    }

    let mut b: List<i32, S> = (0..40).collect();

    for _ in 0..36 {
        b.pop_front();
    }

    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    a.append_elementwise(&mut b);

    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![4, 5, 6, 7, 36, 37, 38, 39]
    );
    assert_eq!(a.storage.slots(), a_slots, "只搬活元素，不动对方的死槽");
    assert_eq!(b.storage.slots(), b_slots, "死槽留在对方那里");
    assert_invariants(&a);
    assert_invariants(&b);

    // 自己装不下 ⇒ 边塞边扩容，仍然正确
    let mut a: List<i32, S> = (0..4).collect();
    let mut b: List<i32, S> = (300..310).collect();

    b.pop_front();

    let len_before = a.len();

    a.append_elementwise(&mut b);

    assert_eq!(a.len(), len_before + 9);
    assert_eq!(
        a.iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 301, 302, 303, 304, 305, 306, 307, 308, 309]
    );
    assert!(b.is_empty());
    assert_invariants(&a);
    assert_invariants(&b);

    // 对方是"空表但带槽位"：什么都不做（对方槽位留在对方那里）
    let mut a: List<i32, S> = (0..5).collect();
    let mut b: List<i32, S> = (0..5).collect();

    b.clear();

    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    a.append_elementwise(&mut b);

    assert_eq!(a.len(), 5);
    assert_eq!(a.storage.slots(), a_slots);
    assert_eq!(b.storage.slots(), b_slots);
    assert_invariants(&a);
    assert_invariants(&b);
}

/// `append` 之后元素的所有权必须完整移交：析构次数不多不少。
fn check_append_drop<S: Storage<Tracked> + Default>() {
    let count = Rc::new(Cell::new(0));

    {
        let mut a: List<Tracked, S> = List::default();
        let mut b: List<Tracked, S> = List::default();

        for _ in 0..5 {
            a.push_back(Tracked(Rc::clone(&count)));
        }

        for _ in 0..7 {
            b.push_back(Tracked(Rc::clone(&count)));
        }

        drop(b.pop_front());

        a.append(&mut b);

        // 12 个元素，搬移过程中不该析构任何一个；只有被 pop 的那个已析构
        assert_eq!(count.get(), 1);
        assert_eq!(a.len(), 11);

        drop(a);
        assert_eq!(count.get(), 12);
    }

    assert_eq!(count.get(), 12);
}

/// `append_elementwise` 之后所有权同样完整移交（逐元素搬，不重复析构）。
fn check_append_elementwise_drop<S: Storage<Tracked> + Default>() {
    let count = Rc::new(Cell::new(0));

    {
        let mut a: List<Tracked, S> = List::default();
        let mut b: List<Tracked, S> = List::default();

        for _ in 0..5 {
            a.push_back(Tracked(Rc::clone(&count)));
        }

        for _ in 0..7 {
            b.push_back(Tracked(Rc::clone(&count)));
        }

        drop(b.pop_front());

        a.append_elementwise(&mut b);

        assert_eq!(count.get(), 1);
        assert_eq!(a.len(), 11);
        assert!(b.is_empty());

        drop(a);
        assert_eq!(count.get(), 12);
    }

    assert_eq!(count.get(), 12);
}

fn check_traits<S: Storage<i32> + Default>() {
    let list: List<i32, S> = (0..3).collect();

    assert_eq!(format!("{list:?}"), "[0, 1, 2]");
    assert_eq!(list, list.clone());
    assert_eq!(format!("{:?}", list.clone()), "[0, 1, 2]");

    let other: List<i32, S> = (0..4).collect();

    assert_ne!(list, other);

    let mut extended: List<i32, S> = List::default();

    extended.extend(0..3);
    assert_eq!(extended, list);

    let from_iter: List<i32, S> = (0..3).collect();

    assert_eq!(from_iter, list);
    assert_eq!(
        (&list).into_iter().copied().collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

// ============================================================
// 容量预留（本库扩展）
// ============================================================

fn check_reserve<S: Storage<i32> + Default>() {
    // 预留后构造，槽位地址在填满预留容量前不应改变。
    let mut list: List<i32, S> = List::with_capacity(64);

    assert!(list.is_empty());

    let data_before = list.storage.layout().data;

    for i in 0..64 {
        list.push_back(i);
    }

    assert_eq!(
        list.storage.layout().data,
        data_before,
        "预留容量内不应该重新分配"
    );
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        (0..64).collect::<Vec<_>>()
    );

    // 创建后也能 reserve。
    let mut list: List<i32, S> = List::default();

    list.reserve(64);

    let data_before = list.storage.layout().data;

    for i in 0..64 {
        list.push_front(i);
    }

    assert_eq!(list.storage.layout().data, data_before);
    assert_eq!(list.len(), 64);
    assert_eq!(list.front().copied(), Some(63));
    assert_eq!(list.back().copied(), Some(0));
}

// ============================================================
// free-list 复用
// ============================================================

fn check_free_list_reuse<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::default();

    for i in 0..10 {
        list.push_back(i);
    }

    let capacity = list.storage.slots();

    for _ in 0..10 {
        list.pop_front();
    }

    assert_eq!(list.storage.slots(), capacity);

    for i in 0..10 {
        list.push_back(i);
    }

    assert_eq!(list.storage.slots(), capacity);
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>()
    );
    assert_invariants(&list);
}

// ============================================================
// 析构
// ============================================================

#[derive(Clone)]
struct Tracked(Rc<Cell<usize>>);

impl Drop for Tracked {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

fn check_drop<S: Storage<Tracked> + Default>() {
    let count = Rc::new(Cell::new(0));

    {
        let mut list: List<Tracked, S> = List::default();

        for _ in 0..10 {
            list.push_back(Tracked(Rc::clone(&count)));
        }

        assert_eq!(count.get(), 0);
        assert_eq!(list.len(), 10);

        drop(list.pop_front().unwrap());
        assert_eq!(count.get(), 1);

        let removed = list.remove(0);
        drop(removed);
        assert_eq!(count.get(), 2);

        list.retain(|_| false);
        assert_eq!(count.get(), 10);
        assert!(list.is_empty());
    }

    assert_eq!(count.get(), 10);
}

fn check_clear<S: Storage<Tracked> + Default>() {
    let count = Rc::new(Cell::new(0));
    let mut list: List<Tracked, S> = List::default();

    for _ in 0..50 {
        list.push_back(Tracked(Rc::clone(&count)));
    }

    let slots = list.storage.slots();

    list.clear();

    // 每个元素恰好析构一次，槽位容量保留，容器回到空状态。
    assert_eq!(count.get(), 50);
    assert!(list.is_empty());
    assert!(list.front().is_none());
    assert!(list.back().is_none());
    assert!(list.iter().next().is_none());
    assert_eq!(list.storage.slots(), slots);

    // 清空后重新填充应复用槽位，不扩容。
    for _ in 0..50 {
        list.push_back(Tracked(Rc::clone(&count)));
    }

    assert_eq!(list.storage.slots(), slots);
    assert_invariants(&list);

    drop(list);

    assert_eq!(count.get(), 100);
}

// ============================================================
// IterMut 的地址计算（元素大小/对齐与 usize 不同）
// ============================================================

fn check_iter_mut_narrow<S: Storage<u8> + Default>() {
    let mut list: List<u8, S> = (0..5u8).collect();

    for value in list.iter_mut() {
        *value += 10;
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![10, 11, 12, 13, 14]
    );

    let mut back: Vec<u8> = Vec::new();

    for value in list.iter_mut().rev() {
        back.push(*value);
    }

    assert_eq!(back, vec![14, 13, 12, 11, 10]);
}

fn check_iter_mut_wide<S: Storage<[u64; 3]> + Default>() {
    let mut list: List<[u64; 3], S> = (0..4).map(|i| [i, i, i]).collect();

    for value in list.iter_mut() {
        value[1] += 100;
    }

    assert_eq!(
        list.iter().map(|value| value[1]).collect::<Vec<_>>(),
        vec![100, 101, 102, 103]
    );
}

/// free-list 复用会让 live 链在内存里变得非顺序（甚至完全逆序），
/// 这是 `IterMut` 地址计算最容易出错的情形。
fn check_iter_mut_scrambled_chain<S: Storage<i32> + Default>() {
    // 1) 删中间两个再插回去：链变成 3 → 0 → 1 → 4 → 5 → 6 → 7 → 2
    let mut list: List<i32, S> = (0..8).collect();

    assert_eq!(list.remove(3), 3);
    assert_eq!(list.remove(2), 2);
    list.push_back(100);
    list.push_front(200);

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![200, 0, 1, 4, 5, 6, 7, 100]
    );

    for value in list.iter_mut() {
        *value += 1;
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![201, 1, 2, 5, 6, 7, 8, 101]
    );

    assert_eq!(
        list.iter_mut()
            .rev()
            .map(|value| *value)
            .collect::<Vec<_>>(),
        vec![101, 8, 7, 6, 5, 2, 1, 201]
    );

    // 2) 清空后重填：槽位按 LIFO 复用，live 链与内存顺序完全相反
    let mut list: List<i32, S> = (0..8).collect();

    list.clear();

    for i in 0..8 {
        list.push_back(i * 10);
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![0, 10, 20, 30, 40, 50, 60, 70]
    );

    for value in list.iter_mut() {
        *value += 1;
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![1, 11, 21, 31, 41, 51, 61, 71]
    );

    // 3) 游标遍历同样要在乱序链上工作
    {
        let mut cursor = list.cursor_front_mut();

        while let Some(value) = cursor.current() {
            *value *= 2;
            cursor.move_next();
        }
    }

    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![2, 22, 42, 62, 82, 102, 122, 142]
    );
}

// ============================================================
// 具体类型的额外检查
// ============================================================

#[test]
fn const_new() {
    const SOA: SoaList<i32> = SoaList::new();
    const PACKED: PackedList<i32> = PackedList::new();
    const AOS: AosList<i32> = AosList::new();

    assert!(SOA.is_empty());
    assert!(PACKED.is_empty());
    assert!(AOS.is_empty());
}

#[test]
#[should_panic]
fn split_off_out_of_bounds_panics() {
    let mut list: SoaList<i32> = (0..3).collect();

    list.split_off(4);
}

#[test]
#[should_panic]
fn remove_out_of_bounds_panics() {
    let mut list: SoaList<i32> = (0..3).collect();

    list.remove(3);
}

// ============================================================
// 三种布局各跑一遍
// ============================================================

#[test]
fn basics() {
    each_layout!(check_basics);
}

#[test]
fn iteration() {
    each_layout!(check_iteration);
}

#[test]
fn cursor_read() {
    each_layout!(check_cursor_read);
}

#[test]
fn cursor_write() {
    each_layout!(check_cursor_write);
}

#[test]
fn cursor_push_pop() {
    each_layout!(check_cursor_push_pop);
}

#[test]
fn cursor_extras() {
    each_layout!(check_cursor_extras);
}

#[test]
fn bulk() {
    each_layout!(check_bulk);
}

#[test]
fn invariants() {
    each_layout!(check_invariants_seq);
}

#[test]
fn append_bulk() {
    each_layout!(check_append);
}

#[test]
fn append_elementwise() {
    each_layout!(check_append_elementwise);
}

#[test]
fn append_drop_semantics() {
    check_append_drop::<Soa<Tracked>>();
    check_append_drop::<Packed<Tracked>>();
    check_append_drop::<Aos<Tracked>>();
    check_append_elementwise_drop::<Soa<Tracked>>();
    check_append_elementwise_drop::<Packed<Tracked>>();
    check_append_elementwise_drop::<Aos<Tracked>>();
}

#[test]
fn traits_stdlib() {
    each_layout!(check_traits);
}

#[test]
fn free_list_reuse() {
    each_layout!(check_free_list_reuse);
}

#[test]
fn reserve_capacity() {
    each_layout!(check_reserve);
}

/// 第 0 个元素的析构会 panic，用来钉住"析构中途 panic"的行为。
struct PanicOnDrop {
    count: Rc<Cell<usize>>,
    id: usize,
}

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        self.count.set(self.count.get() + 1);

        if self.id == 0 {
            panic!("析构里 panic");
        }
    }
}

fn check_drop_panic<S: Storage<PanicOnDrop> + Default>() {
    let count = Rc::new(Cell::new(0));
    let mut list: List<PanicOnDrop, S> = List::default();

    for id in 0..10 {
        list.push_back(PanicOnDrop {
            count: Rc::clone(&count),
            id,
        });
    }

    let before = count.get();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(list)));

    assert!(result.is_err(), "`T::drop` 里的 panic 必须传播出去");
    // 按 `Vec` 的约定：头一个 panic，剩下 9 个**泄漏**（既不析构，也绝不重复析构）。
    assert_eq!(count.get(), before + 1);
}

/// `Drop` 中途 panic 的语义：泄漏剩余元素，但绝不重复析构（照 `Vec::clear` 的约定）。
/// 索引宽度可调：每种宽度跑同一套不变量检查。`IterMut` 的裸地址链接读按宽度分派，
/// 4 字节那条路只有窄索引才会走到 —— 不测就是未覆盖的 UB 面。
fn check_index_width<S: Storage<i32> + Default>(count: usize) {
    let mut list: List<i32, S> = List::with_capacity(4);

    for i in 0..count as i32 {
        list.push_back(i);
    }

    assert_invariants(&list);
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        (0..count as i32).collect::<Vec<_>>()
    );
    assert_eq!(
        list.iter_mut().map(|v| *v).collect::<Vec<_>>(),
        (0..count as i32).collect::<Vec<_>>()
    );

    // 删几个再插回去：free 链 + 空闲标记位都要在窄整数上正确（标记位是 Ix 的最高位）
    for _ in 0..count / 4 {
        let value = list.pop_front().unwrap();

        list.push_back(value);
    }

    list.remove(0);
    list.push_front(-1);
    assert_invariants(&list);

    assert_eq!(list.iter_mut().count(), count);
}

#[test]
fn index_widths_all_behave() {
    // 上限：u8 ⇒ 128、u16 ⇒ 32768 ⇒ 这里分别用 127 / 1000 个元素
    check_index_width::<Soa<i32, u8>>(127);
    check_index_width::<Soa<i32, u16>>(1000);
    check_index_width::<Soa<i32, u32>>(1000);
    check_index_width::<Soa<i32, u64>>(1000);
    check_index_width::<Soa<i32, usize>>(1000);

    check_index_width::<Packed<i32, u8>>(127);
    check_index_width::<Packed<i32, u32>>(1000);
    check_index_width::<Aos<i32, u8>>(127);
    check_index_width::<Aos<i32, u16>>(1000);
    check_index_width::<Aos<i32, u32>>(1000);
}

/// 每槽字节数只由 `T` 与索引宽度决定。
#[test]
fn per_slot_bytes_by_index_width() {
    fn per_slot<T, I: Ix>() -> usize {
        let mut storage: Soa<T, I> = Soa::new();

        storage.grow();

        let layout = storage.layout();

        layout.data_stride + layout.prev_stride + layout.next_stride
    }

    assert_eq!(per_slot::<usize, usize>(), 24);
    assert_eq!(per_slot::<usize, u64>(), 24);
    assert_eq!(per_slot::<usize, u32>(), 16);
    assert_eq!(per_slot::<usize, u16>(), 12);
    assert_eq!(per_slot::<usize, u8>(), 10);
    assert_eq!(per_slot::<[u64; 8], u32>(), 64 + 4 + 4);

    // 默认宽度就是 u32 ⇒ 三种布局在 T=8 上都是 16 B/槽
    let mut soa: Soa<usize> = Soa::new();
    let mut packed: Packed<usize> = Packed::new();
    let mut aos: Aos<usize> = Aos::new();

    soa.grow();
    packed.grow();
    aos.grow();

    for (name, bytes) in [
        (
            "Soa",
            soa.layout().data_stride + soa.layout().prev_stride + soa.layout().next_stride,
        ),
        (
            "Packed",
            packed.layout().data_stride + packed.layout().prev_stride,
        ),
        // Aos 的 data 就在 Node 里 ⇒ 每槽就是 Node 的大小（strides 都是它，别重复算）
        ("Aos", aos.layout().data_stride),
    ] {
        assert_eq!(bytes, 16, "{name} 的默认每槽字节数（T=8, Ix=u32）");
    }
}

/// `Packed` 触顶同样要 panic。
#[test]
#[should_panic(expected = "槽位数超出索引宽度上限")]
fn packed_index_caps_out() {
    let mut list = PackedList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

/// `Aos` 触顶同样要 panic。
#[test]
#[should_panic(expected = "槽位数超出索引宽度上限")]
fn aos_index_caps_out() {
    let mut list = AosList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

/// 触顶要**响亮地 panic**，不能静默截断（否则就是内存错乱）。
#[test]
#[should_panic(expected = "槽位数超出索引宽度上限")]
fn narrow_index_caps_out() {
    let mut list = SoaList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

/// `PhantomData<T>`（`List::marker`）带来的两条**编译期**性质：能编过即成立。
///
/// - 对 `T` **协变**（与三种 `Storage` 一致）⇒ `&'static` 版本能当 `&'a` 版本用；
///   若哪天换成 `PhantomData<fn(T) -> T>` 之类，`covariant` 就编不过了。
/// - `Send` / `Sync` 跟着 `T` 走；`List<MutexGuard<'_>, _>` 之类仍然正确地不是 `Send`。
#[test]
fn auto_traits_and_variance() {
    fn assert_send_sync<X: Send + Sync>() {}

    assert_send_sync::<SoaList<usize>>();
    assert_send_sync::<PackedList<usize>>();
    assert_send_sync::<AosList<usize>>();

    fn covariant<'a>(list: SoaList<&'static str>) -> SoaList<&'a str> {
        list
    }

    let list: SoaList<&'static str> = SoaList::new();

    assert!(covariant(list).is_empty());
}

#[test]
fn drop_panic_leaks_rest() {
    check_drop_panic::<Soa<PanicOnDrop>>();
    check_drop_panic::<Packed<PanicOnDrop>>();
    check_drop_panic::<Aos<PanicOnDrop>>();
}

#[test]
fn drop_semantics() {
    check_drop::<Soa<Tracked>>();
    check_drop::<Packed<Tracked>>();
    check_drop::<Aos<Tracked>>();

    check_clear::<Soa<Tracked>>();
    check_clear::<Packed<Tracked>>();
    check_clear::<Aos<Tracked>>();
}

#[test]
fn iter_mut_offsets() {
    check_iter_mut_narrow::<Soa<u8>>();
    check_iter_mut_narrow::<Packed<u8>>();
    check_iter_mut_narrow::<Aos<u8>>();

    check_iter_mut_wide::<Soa<[u64; 3]>>();
    check_iter_mut_wide::<Packed<[u64; 3]>>();
    check_iter_mut_wide::<Aos<[u64; 3]>>();
}

#[test]
fn iter_mut_scrambled_chain() {
    check_iter_mut_scrambled_chain::<Soa<i32>>();
    check_iter_mut_scrambled_chain::<Packed<i32>>();
    check_iter_mut_scrambled_chain::<Aos<i32>>();
}

// ============================================================
// 两条**独特性**契约（不是性能指标，是语义保证）
// ============================================================

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);

/// 数分配次数的全局分配器（仅测试用）。
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: AllocLayout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: AllocLayout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: AllocLayout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GA: Counting = Counting;

fn count_alloc<F: FnMut()>(mut f: F) -> usize {
    let before = ALLOCS.load(Ordering::Relaxed);

    f();

    ALLOCS.load(Ordering::Relaxed) - before
}

/// 规模：Miri 是解释执行 + 逐字节记录来源，1M 元素会跑成小时级；而这个属性
/// （热路径零分配）**与规模无关**，所以 Miri 下用小规模。
fn churn_scale() -> usize {
    if cfg!(miri) { 64 } else { 1_000_000 }
}

/// **端操作不开分配**：容量备好之后，`pop_front` / `push_back` 走的是
/// free-list 与现成槽位，一次 `malloc` 都不发生（对照：`LinkedList` 每个元素
/// 一次分配；"链块"每 K 个元素一次）。这是"延迟可预测"的基础，
/// 也是 [`List::with_capacity`] + 固定 24 B/槽 的直接推论。
fn check_churn_does_not_allocate<S: Storage<usize> + Default>() {
    let n = churn_scale();
    let mut list: List<usize, S> = List::with_capacity(n);

    for i in 0..n {
        list.push_back(i);
    }

    let allocs = count_alloc(|| {
        for i in 0..n {
            list.pop_front();
            list.push_back(i);
        }
    });

    assert_eq!(allocs, 0, "热身后 churn 不该分配内存");
    assert_eq!(list.len(), n);
}

#[test]
fn churn_does_not_allocate() {
    check_churn_does_not_allocate::<Soa<usize>>();
    check_churn_does_not_allocate::<Packed<usize>>();
    check_churn_does_not_allocate::<Aos<usize>>();

    // 分配计数器本身的体检：std 的链表每个元素一次分配
    let n = churn_scale();
    let mut ll: std::collections::LinkedList<usize> = (0..n).collect();

    let allocs = count_alloc(|| {
        for i in 0..n {
            ll.pop_front();
            ll.push_back(i);
        }
    });

    assert!(allocs > 0, "分配计数器没工作");

    // 对照 2：`fast-list`（slotmap 索引）同样靠复用空槽做到零分配 ——
    // 这条是 PERFORMANCE.md §9 功能对照表里"零分配"那一格的证据。
    let mut fast: fast_list::LinkedList<usize> = fast_list::LinkedList::new();

    for i in 0..n {
        fast.push_back(i);
    }

    let allocs = count_alloc(|| {
        for i in 0..n {
            fast.pop_front();
            fast.push_back(i);
        }
    });

    assert_eq!(allocs, 0, "fast-list 的 churn 也应当零分配");
}

/// **状态可搬运**：整条链的全部状态就是「每槽 `(T, prev, next)` + `head` /
/// `tail` / `free_head` / `free_tail` / `len`」，**没有任何指针**（这正是不变量
/// "数组里每个值都是合法下标、`NIL` 从不写进数组"的用处）。所以把原始数组原样
/// 搬进另一个容器、不做任何链接修正，语义必须完全一致（⇒ 可以直接序列化 /
/// 放进共享内存 / mmap，不需要指针修正）。
fn check_state_is_relocatable<S: Storage<i32> + Default>() {
    let mut src: List<i32, S> = List::with_capacity(16);

    for v in [10, 20, 30, 40, 50] {
        src.push_back(v);
    }

    let removed = src.remove(1); // 留一个空闲槽，逼出非空 free 链
    assert_eq!(removed, 20);
    src.push_back(60);

    let (head, tail, free_head, free_tail, len) =
        (src.head, src.tail, src.free_head, src.free_tail, src.len);
    let slots = src.storage.slots();

    let dump: Vec<(i32, usize, usize)> = (0..slots)
        .map(|i| {
            (
                unsafe { *src.storage.data(i).assume_init_ref() },
                src.storage.prev(i),
                src.storage.next(i),
            )
        })
        .collect();

    // 全新的容器：只写数组与标量，不做任何"链接修正"
    let mut dst: List<i32, S> = List::with_capacity(16);

    while dst.storage.slots() < slots {
        dst.storage.grow();
    }

    for (i, &(data, prev, next)) in dump.iter().enumerate() {
        dst.storage.data_mut(i).write(data);
        dst.storage.set_prev(i, prev);
        dst.storage.set_next(i, next);
    }

    dst.head = head;
    dst.tail = tail;
    dst.free_head = free_head;
    dst.free_tail = free_tail;
    dst.len = len;

    assert_eq!(
        src.iter().copied().collect::<Vec<_>>(),
        dst.iter().copied().collect::<Vec<_>>()
    );
    assert_eq!(
        src.iter().rev().copied().collect::<Vec<_>>(),
        dst.iter().rev().copied().collect::<Vec<_>>()
    );
    assert_eq!(src.free_head, dst.free_head);
    assert_invariants(&dst);
}

#[test]
fn state_is_relocatable() {
    check_state_is_relocatable::<Soa<i32>>();
    check_state_is_relocatable::<Packed<i32>>();
    check_state_is_relocatable::<Aos<i32>>();
}

// ============================================================
// 句柄（Slot）
// ============================================================

/// 句柄的入口/出口、O(1) 增删搬移、失效语义（**不做世代校验**是可接受的取舍）。
fn check_slots<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..5).collect(); // 0 1 2 3 4

    let head = list.front_slot().unwrap();
    let tail = list.back_slot().unwrap();
    let mid = list.at(2).unwrap().slot().unwrap();

    assert_eq!(list.cursor_at(mid).unwrap().current(), Some(&2));
    assert_eq!(list.pos_of(mid), Some(2));
    assert_eq!(list.pos_of(head), Some(0));
    assert_eq!(list.pos_of(tail), Some(4));

    // O(1) 删除：不需要逻辑位置
    assert_eq!(list.remove_slot(mid), Some(2));
    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![0, 1, 3, 4]);
    assert_invariants(&list);

    // 失效句柄：槽位进了 free 链 ⇒ None（且判定时**不碰**空闲槽的 MaybeUninit）
    assert!(list.cursor_at(mid).is_none());
    assert!(list.cursor_at_mut(mid).is_none());
    assert!(list.remove_slot(mid).is_none());
    assert!(list.pos_of(mid).is_none());

    // 槽位被复用后，旧句柄指向**新元素**（不世代校验 = 已知取舍，这里把行为钉住）
    list.push_back(99); // free 链 LIFO ⇒ 正好复用刚空出来的那个槽位
    assert_eq!(
        list.cursor_at(mid).unwrap().current(),
        Some(&99),
        "旧句柄会指向复用后的新元素（没有世代校验）"
    );

    // O(1) 搬移（LRU 原语）
    assert_eq!(list.move_to_front(tail), Some(()));
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![4, 0, 1, 3, 99]
    );
    assert_eq!(list.move_to_back(head), Some(()));
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        vec![4, 1, 3, 99, 0]
    );
    assert_eq!(list.len(), 5);
    assert_invariants(&list);

    // 首尾句柄随搬移更新
    assert_eq!(list.pos_of(list.front_slot().unwrap()), Some(0));
    assert_eq!(list.pos_of(list.back_slot().unwrap()), Some(4));

    // 按句柄进入的游标：位置未知 ⇒ index()/move_steps 按需走链算出来
    let mut cursor = list.cursor_at_mut(list.front_slot().unwrap()).unwrap();
    assert_eq!(cursor.index(), Some(0));
    cursor.move_next();
    assert_eq!(cursor.index(), Some(1));
    assert!(cursor.move_steps(2));
    assert_eq!(cursor.index(), Some(3));

    // 空表边界
    let empty: List<i32, S> = List::default();
    assert_eq!(empty.front_slot(), None);
    assert_eq!(empty.back_slot(), None);
    assert_eq!(empty.iter_slots().count(), 0);
}

#[test]
fn slots() {
    check_slots::<Soa<i32>>();
    check_slots::<Packed<i32>>();
    check_slots::<Aos<i32>>();
}

/// 句柄当别的容器的 key（LRU / `LinkedHashMap` 用法的最小验证）。
fn check_slot_as_key<S: Storage<i32> + Default>() {
    use std::collections::HashMap;

    let mut list: List<i32, S> = (0..4).collect();
    let mut keys: Vec<Slot> = list.iter_slots().map(|(slot, _)| slot).collect();
    let mut map: HashMap<Slot, i32> = list.iter_slots().map(|(slot, &v)| (slot, v)).collect();

    // 用句柄 O(1) 改值、搬位置、删除——全程不需要"第几个"
    for (i, &slot) in keys.iter().enumerate() {
        map.insert(slot, 100 + i as i32);
        assert_eq!(list.move_to_front(slot), Some(()));
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![3, 2, 1, 0]);
    assert_invariants(&list);

    let victim = keys.swap_remove(1);
    assert_eq!(map.remove(&victim), Some(101));
    assert_eq!(list.remove_slot(victim), Some(1));
    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![3, 2, 0]);

    // 剩下的句柄仍然有效，且能取回自己的位置
    for &slot in &keys {
        assert!(list.cursor_at(slot).is_some());
        assert!(list.pos_of(slot).is_some());
    }
    assert_invariants(&list);
}

#[test]
fn slot_as_key() {
    check_slot_as_key::<Soa<i32>>();
    check_slot_as_key::<Packed<i32>>();
    check_slot_as_key::<Aos<i32>>();
}
