//! One test suite shared by all three layouts: the implementation is a single
//! one, so generic functions ×3 suffice.

use super::*;
use crate::storage::{NIL, Storage};
use std::cell::Cell;
use std::rc::Rc;

/// Run `$check` once per layout (`i32` elements).
macro_rules! each_layout {
    ($check:ident) => {
        $check::<Split<i32>>();
        $check::<PackedLinks<i32>>();
        $check::<Nodes<i32>>();
    };
}

// ============================================================
// Basic operations
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
// Iteration
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
// Cursor: std semantics
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

    // move_prev from the first element enters the ghost position.
    cursor.move_prev();
    assert_eq!(cursor.index(), None);
    assert_eq!(cursor.current(), None);
    assert_eq!(cursor.peek_next().copied(), Some(0));
    assert_eq!(cursor.peek_prev().copied(), Some(2));

    // move_next from the ghost position returns to the first element.
    cursor.move_next();
    assert_eq!(cursor.index(), Some(0));

    // move_next from the last element enters the ghost position.
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

    // After insert_before / insert_after the cursor still points at the original node.
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

    // remove_current: the cursor moves to the next element.
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

    // Removing the tail element -> ghost position.
    {
        let back = list.len() - 1;
        let mut cursor = list.at(back).unwrap();

        assert_eq!(cursor.remove_current(), Some(2));
        assert_eq!(cursor.index(), None);
        assert_eq!(cursor.remove_current(), None);
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![10, 20, 25]);

    // At the ghost position, insert_before appends at the back and insert_after inserts at the front.
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

        cursor.move_next(); // points at 1, pos 1

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

    // When pop_front removes the element the cursor points at, the cursor moves to the new front.
    {
        let mut cursor = list.cursor_front_mut();

        assert_eq!(cursor.pop_front(), Some(0));
        assert_eq!(cursor.index(), Some(0));
        assert_eq!(*cursor.current().unwrap(), 1);
    }

    // When pop_back removes the element the cursor points at, the cursor moves to the ghost position.
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
// Bulk operations and traits
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
// Invariants
// ============================================================

/// Whether a slot is on the free chain (test helper). Walk by **count** (the free
/// chain's tail is a self-loop and `NIL` never enters the array, so we cannot "walk until NIL").
fn free_set<S: Storage<T>, T>(list: &List<T, S>) -> Vec<bool> {
    let mut on_free = vec![false; list.storage.slots()];
    let free = list.storage.slots() - list.len();
    let mut slot = list.free_head;

    for step in 0..free {
        assert!(!on_free[slot], "free chain has a cycle (step {step})");
        on_free[slot] = true;
        slot = list.storage.next(slot);
    }

    on_free
}

/// Check every invariant listed in the `List` docs (read-only, no state changes).
fn assert_invariants<T, S: Storage<T>>(list: &List<T, S>) {
    let slots = list.storage.slots();
    let len = list.len();

    // 1) Every value in `next` is a valid index (**no sentinel values**: this is
    //    what lets `append` shift a whole block of indices by `+= base`); `prev`
    //    must be valid only for **live** slots: a free slot's `prev` is stale (its only use is the free bit) and never participates in arithmetic.
    for i in 0..slots {
        assert!(
            list.storage.next(i) < slots,
            "next[{i}] is not a valid index"
        );

        if !list.storage.is_free(i) {
            assert!(
                list.storage.prev(i) < slots,
                "prev[{i}] is not a valid index"
            );
        }
    }

    // 2) Both endpoint fields reset when the list is empty
    assert_eq!(list.head == NIL, len == 0, "head disagrees with len");
    assert_eq!(list.tail == NIL, len == 0, "tail disagrees with len");

    let mut seen = vec![false; slots];
    let mut node = list.head;
    let mut last = NIL;

    // 3) live chain: walking len steps from head covers exactly the live nodes and stops at tail
    for step in 0..len {
        assert!(node != NIL, "live chain is shorter than len");
        assert!(!seen[node], "live chain has a duplicate node (step {step})");
        seen[node] = true;

        let prev = list.storage.prev(node);
        let next = list.storage.next(node);

        // The endpoint "dummy link" fields are required to be mutually inverse only when a real neighbour exists
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
        assert_eq!(last, list.tail, "did not stop at tail after len steps");
    }

    // 4) free chain: length is exactly `slots - len`, disjoint from the live chain, ending at free_tail
    let free = slots - len;

    assert_eq!(
        list.free_head == NIL,
        free == 0,
        "free_head disagrees with the free count"
    );
    assert_eq!(
        list.free_tail == NIL,
        free == 0,
        "free_tail disagrees with the free count"
    );

    let mut node = list.free_head;
    let mut last = NIL;

    for step in 0..free {
        assert!(node != NIL, "free chain is shorter than the free count");
        assert!(
            !seen[node],
            "slot is on both the live and free chains (step {step})"
        );
        seen[node] = true;

        last = node;
        node = list.storage.next(node);
    }

    if free > 0 {
        assert_eq!(last, list.free_tail, "free chain did not reach free_tail");
    }

    assert!(seen.iter().all(|&s| s), "some slot is on neither chain");

    // 5) The free bit must agree exactly with "is on the free chain" (handle
    //    liveness detection relies on it): slots on the free chain → `is_free` true; slots on the live chain → false.
    let on_free_chain = free_set(list);

    for (i, &on_free) in on_free_chain.iter().enumerate() {
        assert_eq!(
            list.storage.is_free(i),
            on_free,
            "slot {i}'s free bit disagrees with the free chain"
        );
    }
}

/// A long sequence of operations; invariants are checked after every step.
fn check_invariants_seq<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::default();
    assert_invariants(&list);

    // Empty -> one node -> many nodes
    list.push_back(0);
    assert_invariants(&list);
    list.push_back(1);
    list.push_front(-1);
    assert_invariants(&list);

    for i in 2..8 {
        list.push_back(i);
        assert_invariants(&list);
    }

    // Endpoint removal (including down to a single element)
    for _ in 0..3 {
        list.pop_front();
        assert_invariants(&list);
    }

    for _ in 0..2 {
        list.pop_back();
        assert_invariants(&list);
    }

    // Middle removal + free-chain reuse
    list.remove(0);
    assert_invariants(&list);
    list.remove(list.len() - 1);
    assert_invariants(&list);

    // retain (may remove the head, the tail, or everything)
    list.retain(|value| *value % 2 == 0);
    assert_invariants(&list);

    // Cursor insertion / removal (including at both ends)
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

    // Insert before the chain head / after the chain tail
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

    // clear: all slots enter the free chain, then refill past the original slot count
    let slots = list.storage.slots();
    list.clear();
    assert_invariants(&list);
    assert_eq!(list.storage.slots(), slots);

    for i in 0..slots + 1 {
        list.push_back(i as i32);
        assert_invariants(&list);
    }

    // Empty it: the endpoint fields must reset to NIL
    while !list.is_empty() {
        list.pop_front();
        assert_invariants(&list);
    }

    // split_off
    let mut list: List<i32, S> = (0..6).collect();
    let tail = list.split_off(2);
    assert_invariants(&list);
    assert_invariants(&tail);

    // Empty it with retain
    let mut list: List<i32, S> = (0..5).collect();
    list.retain(|_| false);
    assert_invariants(&list);
    assert!(list.is_empty());
}

// ============================================================
// append
// ============================================================

fn check_append<S: Storage<i32> + Default>() {
    // Empty ⊕ empty
    let mut a: List<i32, S> = List::default();
    let mut b: List<i32, S> = List::default();

    a.append(&mut b);
    assert!(a.is_empty() && b.is_empty());
    assert_invariants(&a);
    assert_invariants(&b);

    // Empty ⊕ non-empty (`a` has no slots at all: base == 0)
    let mut b: List<i32, S> = (0..5).collect();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    assert_eq!(a.len(), 5);
    assert!(b.is_empty());
    assert_invariants(&a);
    assert_invariants(&b);

    // Non-empty ⊕ empty (the other side has no slots: returns immediately)
    let mut b: List<i32, S> = List::default();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
    assert_invariants(&a);

    // Non-empty ⊕ non-empty: no free slots here and the other side is dense
    // enough ⇒ whole-block move, carrying the other side's free slots along
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
    assert_eq!(
        b.storage.slots(),
        0,
        "slots moved out, capacity stays behind"
    );
    assert_invariants(&a);
    assert_invariants(&b);

    // The moved-in free slots must be reusable (free chains spliced correctly)
    let free = a.storage.slots() - a.len();
    let slots = a.storage.slots();

    assert_eq!(free, a_free + b_free, "wrong number of free slots");

    for i in 0..free {
        a.push_back(i as i32);
    }

    assert_eq!(a.storage.slots(), slots, "not all free slots were reused");

    a.push_back(-1);
    assert!(
        a.storage.slots() > slots,
        "should grow once free slots are exhausted"
    );
    assert_invariants(&a);

    // `self` empty but holding free slots (after clear): still a whole-block move
    let mut a: List<i32, S> = (0..3).collect();

    a.clear();

    let mut b: List<i32, S> = (0..4).collect();

    a.append(&mut b);

    assert_eq!(a.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    assert_eq!(a.len(), 4);
    assert_invariants(&a);

    // Append repeatedly; the chain grows (indices stay consistent across splices)
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

    // Append to an empty list: `base == 0`, but both chains must be spliced correctly
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

    // Cannot fit here (0 free slots < the other side's 9 live elements) ⇒ whole-block splice
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

    // The other side is very sparse: the whole-block splice moves by **slot count** (36 dead slots come along)
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
        "the whole-block splice carries the other side's slots (including dead ones) along"
    );
    assert_eq!(b.storage.slots(), 0);
    assert_invariants(&a);
    assert_invariants(&b);

    // The other side is "empty but holding slots": the whole-block splice carries those free slots along too
    let mut a: List<i32, S> = (0..4).collect();
    let mut b: List<i32, S> = (0..5).collect();

    b.clear();

    let a_slots = a.storage.slots();
    let b_slots = b.storage.slots();

    a.append(&mut b);

    assert_eq!(a.len(), 4);
    assert_eq!(
        a.storage.slots(),
        a_slots + b_slots,
        "free slots are carried along by the whole-block splice"
    );
    assert_eq!(b.storage.slots(), 0);
    assert_invariants(&a);
    assert_invariants(&b);
}

/// `append_elementwise`: moves element by element, preferring this list's own free slots (no growth, the other side's slots stay put).
fn check_append_elementwise<S: Storage<i32> + Default>() {
    // a: 10 slots / 4 live (6 free); b: 3 slots / 3 live ⇒ all fit into the free slots
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
    assert_eq!(
        a.storage.slots(),
        a_slots,
        "reuses free slots, should not grow"
    );
    assert_eq!(a.storage.slots() - a.len(), 6 - 3);
    assert!(b.is_empty());
    assert_eq!(
        b.storage.slots(),
        b_slots,
        "the other side's slots stay with the other side"
    );
    assert_invariants(&a);
    assert_invariants(&b);

    // The other side is sparse (the 1M-slots-100-live case is measured in the benches): move live elements only, leaving its dead slots untouched
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
    assert_eq!(
        a.storage.slots(),
        a_slots,
        "moves live elements only, leaves the other side's dead slots alone"
    );
    assert_eq!(
        b.storage.slots(),
        b_slots,
        "dead slots stay with the other side"
    );
    assert_invariants(&a);
    assert_invariants(&b);

    // Cannot fit here ⇒ grows while filling, still correct
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

    // The other side is "empty but holding slots": nothing happens (its slots stay put)
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

/// After `append` ownership of elements must transfer completely: drop counts neither too high nor too low.
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

        // 12 elements; none should be dropped during the move, only the popped one is already dropped
        assert_eq!(count.get(), 1);
        assert_eq!(a.len(), 11);

        drop(a);
        assert_eq!(count.get(), 12);
    }

    assert_eq!(count.get(), 12);
}

/// After `append_elementwise` ownership transfers completely as well (moves element by element, no double drops).
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
// Capacity reservation (library extension)
// ============================================================

fn check_reserve<S: Storage<i32> + Default>() {
    // Built after reserving; slot addresses must not change until the reserved capacity is filled.
    let mut list: List<i32, S> = List::with_capacity(64);

    assert!(list.is_empty());

    let data_before = list.storage.layout().data;

    for i in 0..64 {
        list.push_back(i);
    }

    assert_eq!(
        list.storage.layout().data,
        data_before,
        "should not reallocate within the reserved capacity"
    );
    assert_eq!(
        list.iter().copied().collect::<Vec<_>>(),
        (0..64).collect::<Vec<_>>()
    );

    // reserve works after construction too.
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
// free-list reuse
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
// Drop
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

    // Each element is dropped exactly once, slot capacity is retained, the container is empty again.
    assert_eq!(count.get(), 50);
    assert!(list.is_empty());
    assert!(list.front().is_none());
    assert!(list.back().is_none());
    assert!(list.iter().next().is_none());
    assert_eq!(list.storage.slots(), slots);

    // Refilling after a clear should reuse slots without growing.
    for _ in 0..50 {
        list.push_back(Tracked(Rc::clone(&count)));
    }

    assert_eq!(list.storage.slots(), slots);
    assert_invariants(&list);

    drop(list);

    assert_eq!(count.get(), 100);
}

// ============================================================
// IterMut address computation (element size/alignment differs from usize)
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

/// free-list reuse makes the live chain non-sequential in memory (even fully reversed),
/// which is the easiest place for `IterMut` address computation to go wrong.
fn check_iter_mut_scrambled_chain<S: Storage<i32> + Default>() {
    // 1) Remove two in the middle and insert them back: the chain becomes 3 → 0 → 1 → 4 → 5 → 6 → 7 → 2
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

    // 2) Clear then refill: slots are reused LIFO, so the live chain is exactly reversed in memory
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

    // 3) Cursor traversal must work on a scrambled chain too
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
// Extra checks for concrete types
// ============================================================

#[test]
fn const_new() {
    const SOA: SplitList<i32> = SplitList::new();
    const PACKED: PackedLinksList<i32> = PackedLinksList::new();
    const AOS: NodesList<i32> = NodesList::new();

    assert!(SOA.is_empty());
    assert!(PACKED.is_empty());
    assert!(AOS.is_empty());
}

#[test]
#[should_panic]
fn split_off_out_of_bounds_panics() {
    let mut list: SplitList<i32> = (0..3).collect();

    list.split_off(4);
}

#[test]
#[should_panic]
fn remove_out_of_bounds_panics() {
    let mut list: SplitList<i32> = (0..3).collect();

    list.remove(3);
}

// ============================================================
// Run once per layout
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
    check_append_drop::<Split<Tracked>>();
    check_append_drop::<PackedLinks<Tracked>>();
    check_append_drop::<Nodes<Tracked>>();
    check_append_elementwise_drop::<Split<Tracked>>();
    check_append_elementwise_drop::<PackedLinks<Tracked>>();
    check_append_elementwise_drop::<Nodes<Tracked>>();
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

/// Element 0's drop panics, pinning down the "panic during drop" behaviour.
struct PanicOnDrop {
    count: Rc<Cell<usize>>,
    id: usize,
}

impl Drop for PanicOnDrop {
    fn drop(&mut self) {
        self.count.set(self.count.get() + 1);

        if self.id == 0 {
            panic!("panic inside Drop");
        }
    }
}

/// Semantics of a panic mid-`Drop`: the remaining elements leak but are never dropped twice
/// (per `Vec::clear`'s convention).
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

    assert!(result.is_err(), "a panic in `T::drop` must propagate out");
    // Per `Vec`'s convention: the first panics and the remaining 9 **leak** (neither dropped nor dropped twice).
    assert_eq!(count.get(), before + 1);
}

/// Index width is configurable: the same invariant checks run for every width. `IterMut`'s raw-address link reads dispatch by width, and
/// the 4-byte path is only reached with narrow indices; leaving it untested would leave an uncovered UB surface.
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

    // Remove a few and insert them back: both the free chain and the free bit must work on narrow integers (the bit is Ix's top bit)
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
    // Caps: u8 ⇒ 128, u16 ⇒ 32768 ⇒ 127 / 1000 elements respectively here
    check_index_width::<Split<i32, u8>>(127);
    check_index_width::<Split<i32, u16>>(1000);
    check_index_width::<Split<i32, u32>>(1000);
    check_index_width::<Split<i32, u64>>(1000);
    check_index_width::<Split<i32, usize>>(1000);

    check_index_width::<PackedLinks<i32, u8>>(127);
    check_index_width::<PackedLinks<i32, u32>>(1000);
    check_index_width::<Nodes<i32, u8>>(127);
    check_index_width::<Nodes<i32, u16>>(1000);
    check_index_width::<Nodes<i32, u32>>(1000);
}

/// Bytes per slot depend only on `T` and the index width.
#[test]
fn per_slot_bytes_by_index_width() {
    fn per_slot<T, I: Ix>() -> usize {
        let mut storage: Split<T, I> = Split::new();

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

    // The default width is `DefaultIx` (`usize` by default; `u32` under the
    // `u32-index` feature) ⇒ with T=8 each slot is 8 + 2 * size_of::<DefaultIx>()
    let per_slot_default = 8 + 2 * core::mem::size_of::<DefaultIx>();

    let mut split: Split<usize> = Split::new();
    let mut links: PackedLinks<usize> = PackedLinks::new();
    let mut nodes: Nodes<usize> = Nodes::new();

    split.grow();
    links.grow();
    nodes.grow();

    for (name, bytes) in [
        (
            "Split",
            split.layout().data_stride + split.layout().prev_stride + split.layout().next_stride,
        ),
        (
            "PackedLinks",
            links.layout().data_stride + links.layout().prev_stride,
        ),
        // Nodes keeps data inside the Node ⇒ a slot is exactly the Node size (all strides equal it, don't count it twice)
        ("Nodes", nodes.layout().data_stride),
    ] {
        assert_eq!(
            bytes, per_slot_default,
            "{name}: default bytes per slot (T=8, Ix=DefaultIx)"
        );
    }
}

/// `PackedLinks` must panic on overflow too.
#[test]
#[should_panic(expected = "slot count exceeds the index width limit")]
fn packed_links_index_caps_out() {
    let mut list = PackedLinksList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

/// `Nodes` must panic on overflow too.
#[test]
#[should_panic(expected = "slot count exceeds the index width limit")]
fn nodes_index_caps_out() {
    let mut list = NodesList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

/// Overflow must **panic loudly**, not silently truncate (which would corrupt memory).
#[test]
#[should_panic(expected = "slot count exceeds the index width limit")]
fn narrow_index_caps_out() {
    let mut list = SplitList::<u8, u8>::new();

    for i in 0..=128u8 {
        list.push_back(i);
    }
}

// ============================================================
// Four "gap-filling" APIs: capacity/shrink, raw state (relocatable), cursor std parity
// ============================================================

/// `into_raw` / `from_raw`: **no element drops**, put back as-is, order and slot identity unchanged.
fn check_raw_roundtrip<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::with_capacity(8);

    for value in [10, 20, 30, 40, 50] {
        list.push_back(value);
    }

    assert_eq!(list.remove(1), 20);
    assert_eq!(list.pop_front(), Some(10));

    let slots_before: Vec<usize> = list.iter_slots().map(|(slot, _)| slot.to_usize()).collect();
    let free_before: Vec<usize> = list.free_slots().map(|slot| slot.to_usize()).collect();

    assert_eq!(
        free_before.len(),
        2,
        "two slots should return to the free chain"
    );
    assert_eq!(list.capacity(), 5);
    assert_eq!(list.len(), 3);

    let raw = list.into_raw();

    assert_eq!(raw.len(), 3);
    assert_eq!(raw.capacity(), 5);
    assert!(raw.head_slot().is_some());
    assert!(raw.tail_slot().is_some());
    assert_eq!(
        raw.free_head_slot().map(|s| s.to_usize()),
        free_before.first().copied()
    );
    assert_eq!(
        raw.free_tail_slot().map(|s| s.to_usize()),
        free_before.last().copied()
    );

    let mut list = unsafe { List::<i32, S>::from_raw(raw) };

    assert_eq!(
        list.iter_slots()
            .map(|(slot, _)| slot.to_usize())
            .collect::<Vec<_>>(),
        slots_before
    );
    assert_eq!(
        list.free_slots()
            .map(|slot| slot.to_usize())
            .collect::<Vec<_>>(),
        free_before
    );
    assert_invariants(&list);
    assert_eq!(list.pop_back(), Some(50));
    assert_invariants(&list);
}

/// `into_raw` frees the array only, **without dropping `T`** (`Vec::into_raw_parts` semantics).
// Miri's leak check will necessarily fire here, as expected.
#[cfg_attr(miri, ignore)]
#[test]
fn into_raw_does_not_drop_elements() {
    let count = Rc::new(Cell::new(0));

    {
        let mut list: SplitList<Tracked> = SplitList::new();

        for _ in 0..5 {
            list.push_back(Tracked(Rc::clone(&count)));
        }

        let raw = list.into_raw();

        assert_eq!(count.get(), 0, "`into_raw` must not drop elements");

        drop(raw);

        assert_eq!(
            count.get(),
            0,
            "dropping the `RawList` must not drop elements either"
        );
    }
}

/// Raw parts + `from_fields` make a complete "serialize → deserialize" round trip.
#[test]
fn raw_parts_roundtrip() {
    // 1) Take the state
    let mut split: SplitList<i32, u32> = SplitList::with_capacity(8);

    for value in [1, 2, 3, 4, 5, 6] {
        split.push_back(value);
    }

    assert_eq!(split.remove(2), 3);

    let expected: Vec<i32> = split.iter().copied().collect();
    let expected_slots: Vec<usize> = split.iter_slots().map(|(s, _)| s.to_usize()).collect();
    let expected_free: Vec<usize> = split.free_slots().map(|s| s.to_usize()).collect();

    let raw = split.into_raw();

    // 2) Serialize: copy the arrays out byte by byte + record the five numbers
    let (data, prev, next) = raw.storage().as_parts();
    let (data, prev, next) = (data.to_vec(), prev.to_vec(), next.to_vec());
    let fields = (
        raw.head_slot(),
        raw.tail_slot(),
        raw.free_head_slot(),
        raw.free_tail_slot(),
        raw.len(),
    );
    drop(raw);

    // 3) Deserialize: rebuild the storage, then reassemble the List
    let storage = unsafe { Split::<i32, u32>::from_parts(data, prev, next) };
    let (head, tail, free_head, free_tail, len) = fields;
    let list = unsafe {
        List::<i32, Split<i32, u32>>::from_raw(RawList::from_fields(
            storage, head, tail, free_head, free_tail, len,
        ))
    };

    assert_invariants(&list);
    assert_eq!(list.iter().copied().collect::<Vec<_>>(), expected);
    assert_eq!(
        list.iter_slots()
            .map(|(s, _)| s.to_usize())
            .collect::<Vec<_>>(),
        expected_slots
    );
    assert_eq!(
        list.free_slots().map(|s| s.to_usize()).collect::<Vec<_>>(),
        expected_free
    );
}

/// `capacity` / `shrink_to_fit`: slot count and handles are **unchanged** by shrinking.
fn check_capacity<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = List::with_capacity(64);

    for value in 0..10 {
        list.push_back(value);
    }

    assert_eq!(list.remove(0), 0);

    let slots_before: Vec<usize> = list.iter_slots().map(|(s, _)| s.to_usize()).collect();
    let capacity = list.capacity();

    assert_eq!(capacity, 10);
    assert!(list.len() <= list.capacity());

    list.shrink_to_fit();

    assert_eq!(
        list.capacity(),
        capacity,
        "shrinking returns only excess capacity; the slot count is unchanged"
    );
    assert_eq!(list.len(), 9);
    assert_eq!(
        list.iter_slots()
            .map(|(s, _)| s.to_usize())
            .collect::<Vec<_>>(),
        slots_before,
        "handles are unaffected by shrinking"
    );
    assert_invariants(&list);
}

#[test]
fn capacity_and_shrink() {
    check_capacity::<Split<i32>>();
    check_capacity::<PackedLinks<i32>>();
    check_capacity::<Nodes<i32>>();
}

#[test]
fn raw_state_roundtrips() {
    check_raw_roundtrip::<Split<i32>>();
    check_raw_roundtrip::<PackedLinks<i32>>();
    check_raw_roundtrip::<Nodes<i32>>();
}

/// The four cursor std-parity operations, compared against a `Vec` model.
#[test]
fn cursor_std_parity() {
    let mut list: SplitList<i32> = (1..=3).collect();
    let other: SplitList<i32> = (10..=12).collect();

    // The cursor holds a mutable borrow of `list` for this scope; only after it can `list` be read directly again
    {
        // splice_before: attach in original order before 2; the cursor stays on 2
        let mut cursor = list.at(1).unwrap();

        cursor.splice_before(other);

        assert_eq!(*cursor.current().unwrap(), 2);
        assert_eq!(
            cursor.as_list().iter().copied().collect::<Vec<_>>(),
            vec![1, 10, 11, 12, 2, 3]
        );

        // splice_after: attach after 2, order preserved
        let other: SplitList<i32> = (20..=21).collect();

        cursor.splice_after(other);

        assert_eq!(*cursor.current().unwrap(), 2);
        assert_eq!(
            cursor.as_list().iter().copied().collect::<Vec<_>>(),
            vec![1, 10, 11, 12, 2, 20, 21, 3]
        );

        // remove_current_as_list: detach into a single-element list; the cursor moves to 20
        let one = cursor.remove_current_as_list().unwrap();

        assert_eq!(one.iter().copied().collect::<Vec<_>>(), vec![2]);
        assert_eq!(one.len(), 1);
        assert_eq!(*cursor.current().unwrap(), 20);
        assert_eq!(
            cursor.as_list().iter().copied().collect::<Vec<_>>(),
            vec![1, 10, 11, 12, 20, 21, 3]
        );

        // split_after: the part after 20 is detached
        let tail = cursor.split_after();

        assert_eq!(tail.iter().copied().collect::<Vec<_>>(), vec![21, 3]);
        assert_eq!(
            cursor.as_list().iter().copied().collect::<Vec<_>>(),
            vec![1, 10, 11, 12, 20]
        );
        assert_eq!(*cursor.current().unwrap(), 20);

        // split_before: the part before 20 is detached; the cursor now points at the chain head
        let head = cursor.split_before();

        assert_eq!(
            head.iter().copied().collect::<Vec<_>>(),
            vec![1, 10, 11, 12]
        );
        assert_eq!(cursor.index(), Some(0));
        assert_eq!(*cursor.current().unwrap(), 20);
    }

    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![20]);
    assert_invariants(&list);
}

/// Two **compile-time** properties brought by `PhantomData<T>` (`List::marker`): if it compiles, it holds.
///
/// - **Covariant** in `T` (consistent with all three `Storage` types) ⇒ a `&'static` version can be used where `&'a` is expected;
///   if it were ever changed to something like `PhantomData<fn(T) -> T>`, `covariant` would stop compiling.
/// - `Send` / `Sync` follow `T`; something like `List<MutexGuard<'_>, _>` correctly remains non-`Send`.
#[test]
fn auto_traits_and_variance() {
    fn assert_send_sync<X: Send + Sync>() {}

    assert_send_sync::<SplitList<usize>>();
    assert_send_sync::<PackedLinksList<usize>>();
    assert_send_sync::<NodesList<usize>>();

    fn covariant<'a>(list: SplitList<&'static str>) -> SplitList<&'a str> {
        list
    }

    let list: SplitList<&'static str> = SplitList::new();

    assert!(covariant(list).is_empty());
}

// A panic mid-drop ⇒ the remaining elements are unreachable anyway, and this test asserts exactly that "they leak" ⇒ miri's leak check will necessarily fire, as expected.
#[cfg_attr(miri, ignore)]
#[test]
fn drop_panic_leaks_rest() {
    check_drop_panic::<Split<PanicOnDrop>>();
    check_drop_panic::<PackedLinks<PanicOnDrop>>();
    check_drop_panic::<Nodes<PanicOnDrop>>();
}

#[test]
fn drop_semantics() {
    check_drop::<Split<Tracked>>();
    check_drop::<PackedLinks<Tracked>>();
    check_drop::<Nodes<Tracked>>();

    check_clear::<Split<Tracked>>();
    check_clear::<PackedLinks<Tracked>>();
    check_clear::<Nodes<Tracked>>();
}

#[test]
fn iter_mut_offsets() {
    check_iter_mut_narrow::<Split<u8>>();
    check_iter_mut_narrow::<PackedLinks<u8>>();
    check_iter_mut_narrow::<Nodes<u8>>();

    check_iter_mut_wide::<Split<[u64; 3]>>();
    check_iter_mut_wide::<PackedLinks<[u64; 3]>>();
    check_iter_mut_wide::<Nodes<[u64; 3]>>();
}

#[test]
fn iter_mut_scrambled_chain() {
    check_iter_mut_scrambled_chain::<Split<i32>>();
    check_iter_mut_scrambled_chain::<PackedLinks<i32>>();
    check_iter_mut_scrambled_chain::<Nodes<i32>>();
}

// ============================================================
// Two **uniqueness** contracts (semantic guarantees, not performance claims)
// ============================================================

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};

// Allocations are counted **per thread**: the test harness runs the other tests in
// parallel, and a single global counter let their allocations leak into the measured
// region (which made this test fail on Windows).
thread_local! {
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

/// Bump this thread's counter; called from the global allocator.
///
/// `try_with` because the allocator can also run during thread-local teardown, where
/// `with` would panic.
fn record_alloc() {
    let _ = ALLOCS.try_with(|count| count.set(count.get() + 1));
}

/// This thread's allocation count so far.
fn allocs() -> usize {
    ALLOCS.with(Cell::get)
}

/// Global allocator that counts allocations (test-only).
struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: AllocLayout) -> *mut u8 {
        record_alloc();
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: AllocLayout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: AllocLayout, n: usize) -> *mut u8 {
        record_alloc();
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GA: Counting = Counting;

fn count_alloc<F: FnMut()>(mut f: F) -> usize {
    let before = allocs();

    f();

    allocs() - before
}

/// Scale: Miri interprets and records provenance byte by byte, so 1M elements would take hours;
/// and this property (zero allocations on the hot path) is **scale-independent**, so a small scale is used under Miri.
fn churn_scale() -> usize {
    if cfg!(miri) { 64 } else { 1_000_000 }
}

/// **Endpoint operations allocate nothing**: once capacity is in place, `pop_front` / `push_back` use the
/// free list and existing slots, so not a single `malloc` occurs (compare: `LinkedList` allocates per element;
/// a "chain block" allocates once per K elements). This underpins "predictable latency" and
/// follows directly from [`List::with_capacity`] plus a fixed 24 B/slot.
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

    assert_eq!(allocs, 0, "churn should not allocate after warm-up");
    assert_eq!(list.len(), n);
}

#[test]
fn churn_does_not_allocate() {
    check_churn_does_not_allocate::<Split<usize>>();
    check_churn_does_not_allocate::<PackedLinks<usize>>();
    check_churn_does_not_allocate::<Nodes<usize>>();

    // Sanity check the allocation counter itself: std's list allocates once per element
    let n = churn_scale();
    let mut ll: std::collections::LinkedList<usize> = (0..n).collect();

    let allocs = count_alloc(|| {
        for i in 0..n {
            ll.pop_front();
            ll.push_back(i);
        }
    });

    assert!(allocs > 0, "the allocation counter is not working");

    // Control 2: `fast-list` (slotmap-indexed) likewise reaches zero allocation by reusing free slots;
    // this is the evidence for the "zero allocation" cell of the PERFORMANCE.md §7.2 feature comparison table.
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

    assert_eq!(allocs, 0, "fast-list churn should be zero-allocation too");
}

/// **Relocatable state**: the whole state of the chain is just "per slot `(T, prev, next)` + `head` /
/// `tail` / `free_head` / `free_tail` / `len`" with **no pointers at all** (this is what the invariant
/// "every value in the array is a valid index, `NIL` is never written into the array" buys). So moving the raw arrays
/// as-is into another container with no link fixup must yield exactly the same semantics (⇒ direct serialization /
/// shared memory / mmap, no pointer fixup needed).
fn check_state_is_relocatable<S: Storage<i32> + Default>() {
    let mut src: List<i32, S> = List::with_capacity(16);

    for v in [10, 20, 30, 40, 50] {
        src.push_back(v);
    }

    let removed = src.remove(1); // leave one free slot to force a non-empty free chain
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

    // A brand-new container: write only the arrays and scalars, no "link fixup"
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
    check_state_is_relocatable::<Split<i32>>();
    check_state_is_relocatable::<PackedLinks<i32>>();
    check_state_is_relocatable::<Nodes<i32>>();
}

// ============================================================
// Handles (Slot)
// ============================================================

/// Handle entry/exit, O(1) insert/remove/move, and invalidation semantics (**no generation check** is an accepted trade-off).
fn check_slots<S: Storage<i32> + Default>() {
    let mut list: List<i32, S> = (0..5).collect(); // 0 1 2 3 4

    let head = list.front_slot().unwrap();
    let tail = list.back_slot().unwrap();
    let mid = list.at(2).unwrap().slot().unwrap();

    assert_eq!(list.cursor_at(mid).unwrap().current(), Some(&2));
    assert_eq!(list.pos_of(mid), Some(2));
    assert_eq!(list.pos_of(head), Some(0));
    assert_eq!(list.pos_of(tail), Some(4));

    // O(1) removal: no logical position needed
    assert_eq!(list.remove_slot(mid), Some(2));
    assert_eq!(list.iter().copied().collect::<Vec<_>>(), vec![0, 1, 3, 4]);
    assert_invariants(&list);

    // Stale handle: the slot entered the free chain ⇒ None (and the check **does not touch** the free slot's MaybeUninit)
    assert!(list.cursor_at(mid).is_none());
    assert!(list.cursor_at_mut(mid).is_none());
    assert!(list.remove_slot(mid).is_none());
    assert!(list.pos_of(mid).is_none());

    // Once the slot is reused, the old handle points at the **new element** (no generation check = known trade-off; this pins the behaviour)
    list.push_back(99); // free chain is LIFO ⇒ it reuses exactly the slot just freed
    assert_eq!(
        list.cursor_at(mid).unwrap().current(),
        Some(&99),
        "a stale handle points at the new element after reuse (no generation check)"
    );

    // O(1) move (LRU primitive)
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

    // Front/back handles update with the move
    assert_eq!(list.pos_of(list.front_slot().unwrap()), Some(0));
    assert_eq!(list.pos_of(list.back_slot().unwrap()), Some(4));

    // Cursor entered by handle: position unknown ⇒ index()/move_steps walk the chain on demand
    let mut cursor = list.cursor_at_mut(list.front_slot().unwrap()).unwrap();
    assert_eq!(cursor.index(), Some(0));
    cursor.move_next();
    assert_eq!(cursor.index(), Some(1));
    assert!(cursor.move_steps(2));
    assert_eq!(cursor.index(), Some(3));

    // Empty-list boundary
    let empty: List<i32, S> = List::default();
    assert_eq!(empty.front_slot(), None);
    assert_eq!(empty.back_slot(), None);
    assert_eq!(empty.iter_slots().count(), 0);
}

#[test]
fn slots() {
    check_slots::<Split<i32>>();
    check_slots::<PackedLinks<i32>>();
    check_slots::<Nodes<i32>>();
}

/// Handle as a key in another container (minimal check of the LRU / `LinkedHashMap` use case).
fn check_slot_as_key<S: Storage<i32> + Default>() {
    use std::collections::HashMap;

    let mut list: List<i32, S> = (0..4).collect();
    let mut keys: Vec<Slot> = list.iter_slots().map(|(slot, _)| slot).collect();
    let mut map: HashMap<Slot, i32> = list.iter_slots().map(|(slot, &v)| (slot, v)).collect();

    // Change the value, move the position, and remove in O(1) via the handle, with no index lookup anywhere
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

    // The remaining handles are still valid and can recover their positions
    for &slot in &keys {
        assert!(list.cursor_at(slot).is_some());
        assert!(list.pos_of(slot).is_some());
    }
    assert_invariants(&list);
}

#[test]
fn slot_as_key() {
    check_slot_as_key::<Split<i32>>();
    check_slot_as_key::<PackedLinks<i32>>();
    check_slot_as_key::<Nodes<i32>>();
}

/// Both paths of the hybrid `clear` (`2 * len >= slots` scans, otherwise walks the chain) must: drop only
/// **live** elements, preserve the invariants, and return every slot to the free chain so all can be reused.
///
/// `SLOTS = 16` ⇒ `live >= 8` scans, `live < 8` walks the chain, covering both sides of the threshold.
fn check_clear_paths<S: Storage<Tracked> + Default>() {
    const SLOTS: usize = 16;

    for live in 0..=SLOTS {
        let count = Rc::new(Cell::new(0));
        let mut list: List<Tracked, S> = List::default();

        for _ in 0..SLOTS {
            list.push_back(Tracked(Rc::clone(&count)));
        }

        for _ in live..SLOTS {
            drop(list.pop_front());
        }

        assert_eq!(list.len(), live);
        assert_eq!(list.storage.slots(), SLOTS);
        assert_eq!(count.get(), SLOTS - live);

        list.clear();
        assert_eq!(list.len(), 0);
        assert_eq!(
            count.get(),
            SLOTS,
            "live = {live}: each element is dropped exactly once"
        );
        assert_invariants(&list);

        // Every cleared slot must be reusable (free chain complete; order is irrelevant)
        for _ in 0..SLOTS {
            list.push_back(Tracked(Rc::clone(&count)));
        }

        assert_eq!(
            list.len(),
            SLOTS,
            "live = {live}: not all slots returned to the free chain"
        );
        assert_invariants(&list);
    }
}

#[test]
fn clear_paths() {
    check_clear_paths::<Split<Tracked>>();
    check_clear_paths::<PackedLinks<Tracked>>();
    check_clear_paths::<Nodes<Tracked>>();
}

// ============================================================
// `clear`: **walk the chain** vs **scan slots in order + test `prev`'s top bit** (referenced by `List::clear`'s docs)
//
//   cargo test --release -- --ignored --nocapture probe_clear_vs_scan
//
// Not reusing criterion: this wants a **density curve** (live/slots swept from 1 to 0.001) to find the crossover,
// plus the free-chain locality difference between the two clears. The `Drop` comparison lives in the benches' `clear_drop` group.
// ============================================================
#[cfg(test)]
mod probe_clear_vs_scan {
    use super::*;
    use std::hint::black_box;
    use std::time::Instant;

    const PN: usize = 1_000_000;
    const PROUNDS: usize = 9;

    /// min over 9 rounds; `make` is outside the timing, only `f` is measured.
    fn p_bench_state<St, M: FnMut() -> St, F: FnMut(&mut St)>(mut make: M, mut f: F) -> f64 {
        f(&mut make()); // warm-up
        let mut samples = Vec::with_capacity(PROUNDS);
        for _ in 0..PROUNDS {
            let mut st = make();
            let t = Instant::now();
            f(&mut st);
            samples.push(t.elapsed().as_secs_f64() * 1e3);
            black_box(&mut st);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        samples[0] // min: this repository's reading convention
    }

    fn p_trim<T, S: Storage<T>>(list: &mut List<T, S>, live: usize) {
        while list.len() > live {
            list.pop_front();
        }
    }

    /// Chain-walking clear: the equivalent of the `2 * len < slots` branch of the hybrid implementation (touches only live slots).
    ///
    /// The probe **cannot** use `clear()` to represent chain walking: it is now hybrid and scans when dense.
    fn p_walk_clear<T, S: Storage<T>>(list: &mut List<T, S>) {
        while list.len > 0 {
            let slot = list.head;

            // Take the next one first: `push_free` rewrites this slot's own `next`
            list.head = list.storage.next(slot);

            unsafe { list.storage.data_mut(slot).assume_init_drop() };

            list.len -= 1;
            list.push_free(slot);
        }

        list.head = NIL;
        list.tail = NIL;
    }

    /// Scanning clear: walk **all slots** in memory order, using `is_free` to tell live from free.
    /// Uses the same `push_free` (push to the chain head) as the production code; only the `drop_live = false` variant is
    /// probe-specific (no drops, isolating the cost of the bit-flag pass over memory).
    fn p_scan_clear<T, S: Storage<T>>(list: &mut List<T, S>, drop_live: bool) {
        let slots = list.storage.slots();

        for slot in 0..slots {
            if list.storage.is_free(slot) {
                continue;
            }

            if drop_live {
                unsafe { list.storage.data_mut(slot).assume_init_drop() };
            }

            list.push_free(slot);
        }

        list.head = NIL;
        list.tail = NIL;
        list.len = 0;
    }

    /// 64 B payload with Drop glue (same shape as in the benches: really read a field to pin the drop).
    struct P64([u64; 8]);

    impl P64 {
        #[inline]
        fn new(v: usize) -> Self {
            Self([v as u64; 8])
        }
    }

    impl Drop for P64 {
        #[inline]
        fn drop(&mut self) {
            black_box(self.0[0]);
        }
    }

    fn p_build_usize<S: Storage<usize> + Default>(live: usize) -> List<usize, S> {
        let mut l: List<usize, S> = List::default();
        for i in 0..PN {
            l.push_back(black_box(i));
        }
        p_trim(&mut l, live);
        l
    }

    fn p_build_glue<S: Storage<P64> + Default>(live: usize) -> List<P64, S> {
        let mut l: List<P64, S> = List::default();
        for i in 0..PN {
            l.push_back(P64::new(black_box(i)));
        }
        p_trim(&mut l, live);
        l
    }

    macro_rules! probe_layout {
        ($label:literal, $S:ty, $SG:ty) => {{
            println!("--- {} ---", $label);
            println!(
                "{:<28} {:>12} {:>12} {:>12} {:>9}",
                "shape", "walk live", "scan slots", "walk/scan", "live/slots"
            );

            // No Drop glue: density curve
            // The curve is swept in **descending** order ⇒ later assignments have lower density;
            // two points are tracked:
            //   `tie`   = smallest density where scanning breaks even (≈ crossover); `worth` = smallest density where scanning is ≥1.5× faster (the threshold that genuinely justifies switching)
            let mut tie: Option<f64> = None;
            let mut worth: Option<f64> = None;

            for &(name, live) in &[
                ("dense 1M/1M", PN),
                ("0.75 750k/1M", 750_000),
                ("0.5 500k/1M", 500_000),
                ("0.25 250k/1M", 250_000),
                ("0.1 100k/1M", 100_000),
                ("0.01 10k/1M", 10_000),
                ("0.001 1k/1M", 1_000),
            ] {
                let walk = p_bench_state(|| p_build_usize::<$S>(live), |l| p_walk_clear(l));
                let scan = p_bench_state(
                    || p_build_usize::<$S>(live),
                    |l| p_scan_clear(l, true),
                );
                let density = live as f64 / PN as f64;

                let ratio = walk / scan;

                if ratio >= 1.0 {
                    tie = Some(density);
                }

                if ratio >= 1.5 {
                    worth = Some(density);
                }

                println!(
                    "{:<28} {:>10.3} ms {:>10.3} ms {:>11.2}x {:>9.3}",
                    name,
                    walk,
                    scan,
                    walk / scan,
                    density
                );
            }

            let show = |v: Option<f64>| match v {
                Some(d) => format!("live/slots ≳ {d:.2}"),
                None => "not worth it across the whole curve".to_string(),
            };
            println!("{:<28} ⇒ {}", "crossover", show(tie));
            println!("{:<28} ⇒ {}\n", "worth switching (≥1.5×)", show(worth));

            // No glue: only scan prev to classify, **no drops** (isolating the cost of the bit-flag pass over memory)
            let walk = p_bench_state(|| p_build_usize::<$S>(PN), |l| p_walk_clear(l));
            let scan_nodrop =
                p_bench_state(|| p_build_usize::<$S>(PN), |l| p_scan_clear(l, false));
            println!(
                "{:<28} {:>10.3} ms {:>10.3} ms {:>11.2}x {:>9.3}",
                "dense: scan without dropping",
                walk,
                scan_nodrop,
                walk / scan_nodrop,
                1.0
            );

            // With Drop glue
            for &(name, live) in &[("glue dense 1M/1M", PN), ("glue 0.001 1k/1M", 1_000)] {
                let walk = p_bench_state(|| p_build_glue::<$SG>(live), |l| p_walk_clear(l));
                let scan = p_bench_state(
                    || p_build_glue::<$SG>(live),
                    |l| p_scan_clear(l, true),
                );
                println!(
                    "{:<28} {:>10.3} ms {:>10.3} ms {:>11.2}x {:>9.3}",
                    name,
                    walk,
                    scan,
                    walk / scan,
                    live as f64 / PN as f64
                );
            }

            // Two `Drop` implementations: current (walk the live chain, drop in place) vs old (clear first, then drop).
            // Second-order effect: the two clears leave the free chain in different orders, affecting refill locality.
            let refill_walk = p_bench_state(
                || {
                    let mut l = p_build_usize::<$S>(PN);
                    p_walk_clear(&mut l);
                    l
                },
                |l| {
                    for i in 0..PN {
                        l.push_back(black_box(i));
                    }
                },
            );
            let refill_scan = p_bench_state(
                || {
                    let mut l = p_build_usize::<$S>(PN);
                    p_scan_clear(&mut l, true);
                    l
                },
                |l| {
                    for i in 0..PN {
                        l.push_back(black_box(i));
                    }
                },
            );
            println!(
                "{:<28} {:>10.3} ms {:>10.3} ms {:>11.2}x {:>9.3}",
                "refill: after walk/after scan",
                refill_walk,
                refill_scan,
                refill_walk / refill_scan,
                1.0
            );

            println!();
        }};
    }

    #[test]
    #[ignore]
    fn probe_clear_vs_scan() {
        // Self-check first: the scanning version's semantics match `clear` (invariants + emptied + reusable)
        {
            let mut l: List<usize, Split<usize, u32>> = p_build_usize(1234);
            p_scan_clear(&mut l, true);
            assert_eq!(l.len(), 0);
            assert_invariants(&l);
            for i in 0..100 {
                l.push_back(i);
            }
            assert_eq!(
                l.iter().copied().collect::<Vec<_>>(),
                (0..100).collect::<Vec<_>>()
            );
            assert_invariants(&l);
            println!("scan self-check: invariants pass, reusable\n");
        }

        println!("PN = {PN} slots, min over {PROUNDS} rounds\n");
        probe_layout!("Split (u32 index)", Split<usize, u32>, Split<P64, u32>);
        probe_layout!("Split (usize index = u64)", Split<usize, usize>, Split<P64, usize>);
        probe_layout!("PackedLinks (u32 index)", PackedLinks<usize, u32>, PackedLinks<P64, u32>);
        probe_layout!(
            "PackedLinks (usize index)",
            PackedLinks<usize, usize>,
            PackedLinks<P64, usize>
        );
        probe_layout!("Nodes (u32 index)", Nodes<usize, u32>, Nodes<P64, u32>);
        probe_layout!("Nodes (usize index)", Nodes<usize, usize>, Nodes<P64, usize>);
    }
}
