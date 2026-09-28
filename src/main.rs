#![feature(linked_list_cursors)]

mod my_deque;
mod my_deque_aos;
mod my_deque_packed;

use my_deque::MyDeque;
use my_deque_aos::MyDequeAos;
use my_deque_packed::MyDequePacked;

use fast_list::LinkedList as FastLinkedList;

use std::{
    collections::{LinkedList, VecDeque},
    hint::black_box,
    time::{Duration, Instant},
};

const N: usize = 1_000_000;

const ROUNDS: usize = 10;

const LOOKUPS: usize = 100;

const MIDDLE_OPS: usize = 1_000;

const CHURN_OPS: usize = 1_000_000;

const RANDOM_OPS: usize = 100;

const CURSOR_UPDATE_OPS: usize = 1_000_000;

const LARGE_N: usize = 250_000;

// ============================================================
// Benchmark infrastructure
// ============================================================

fn benchmark<F: FnMut()>(mut f: F) -> Duration {
    // Warmup.
    f();

    let mut samples = Vec::with_capacity(ROUNDS);

    for _ in 0..ROUNDS {
        let start = Instant::now();

        f();

        samples.push(start.elapsed());
    }

    samples.sort_unstable();

    samples[ROUNDS / 2]
}

// ============================================================
// Deterministic pseudo-random input
// ============================================================

#[inline]
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

// ============================================================
// std::LinkedList cursor positioning
// ============================================================

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

// ============================================================
// End operations
// ============================================================

fn mydeque_push_back_pop_front() {
    let mut deque = MyDeque::new();

    for i in 0..N {
        let _ = deque.push_back(black_box(i));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn aosdeque_push_back_pop_front() {
    let mut deque = MyDequeAos::new();

    for i in 0..N {
        let _ = deque.push_back(black_box(i));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn packeddeque_push_back_pop_front() {
    let mut deque = MyDequePacked::new();

    for i in 0..N {
        let _ = deque.push_back(black_box(i));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

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

fn mydeque_push_front_pop_back() {
    let mut deque = MyDeque::new();

    for i in 0..N {
        let _ = deque.push_front(black_box(i));
    }

    while let Some(value) = deque.pop_back() {
        black_box(value);
    }
}

fn aosdeque_push_front_pop_back() {
    let mut deque = MyDequeAos::new();

    for i in 0..N {
        let _ = deque.push_front(black_box(i));
    }

    while let Some(value) = deque.pop_back() {
        black_box(value);
    }
}

fn packeddeque_push_front_pop_back() {
    let mut deque = MyDequePacked::new();

    for i in 0..N {
        let _ = deque.push_front(black_box(i));
    }

    while let Some(value) = deque.pop_back() {
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

// ============================================================
// Constructors
// ============================================================

fn make_mydeque() -> MyDeque<usize> {
    let mut deque = MyDeque::new();

    for i in 0..N {
        deque.push_back(i);
    }

    deque
}

fn make_aosdeque() -> MyDequeAos<usize> {
    let mut deque = MyDequeAos::new();

    for i in 0..N {
        deque.push_back(i);
    }

    deque
}

fn make_packeddeque() -> MyDequePacked<usize> {
    let mut deque = MyDequePacked::new();

    for i in 0..N {
        deque.push_back(i);
    }

    deque
}

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

// ============================================================
// Pure iteration
// ============================================================

fn mydeque_iter(deque: &MyDeque<usize>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

fn aosdeque_iter(deque: &MyDequeAos<usize>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

fn packeddeque_iter(deque: &MyDequePacked<usize>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

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

// ============================================================
// Middle positional access
//
// MyDeque:     at(middle)
// MyDequeAos:  at(middle)
// VecDeque:    index[middle]
// LinkedList:  iter().nth(middle)
// FastList:    nth(middle) + get(index)
//
// For FastList, nth() itself performs O(N) traversal, just like
// the other linked structures.
// ============================================================

fn mydeque_at_middle(deque: &mut MyDeque<usize>) {
    let mut sum = 0usize;

    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let cursor = deque.at(black_box(middle)).unwrap();

        sum = sum.wrapping_add(*cursor.value());
    }

    black_box(sum);
}

fn aosdeque_at_middle(deque: &mut MyDequeAos<usize>) {
    let mut sum = 0usize;

    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let cursor = deque.at(black_box(middle)).unwrap();

        sum = sum.wrapping_add(*cursor.value());
    }

    black_box(sum);
}

fn packeddeque_at_middle(deque: &mut MyDequePacked<usize>) {
    let mut sum = 0usize;
    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let cursor = deque.at(black_box(middle)).unwrap();
        sum = sum.wrapping_add(*cursor.value());
    }

    black_box(sum);
}

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

// ============================================================
// Known-cursor / known-index local insertion/removal
//
// MyDeque / MyDequeAos:
//     cursor already positioned at middle.
//
// std LinkedList:
//     CursorMut already positioned at middle.
//
// fast-list:
//     LinkedListIndex already positioned at middle.
//
// VecDeque:
//     middle index.
//
// The positioning step itself is NOT timed.
// ============================================================

fn mydeque_insert_before_remove(deque: &mut MyDeque<usize>) {
    let middle = N / 2;

    let mut cursor = deque.at(middle).unwrap();

    for i in 0..MIDDLE_OPS {
        cursor = cursor.insert_before(black_box(i));

        cursor = cursor.next().unwrap();

        let (_, next_cursor) = cursor.remove();

        cursor = next_cursor.unwrap();
    }

    black_box(cursor.pos());
}

fn aosdeque_insert_before_remove(deque: &mut MyDequeAos<usize>) {
    let middle = N / 2;

    let mut cursor = deque.at(middle).unwrap();

    for i in 0..MIDDLE_OPS {
        cursor = cursor.insert_before(black_box(i));

        cursor = cursor.next().unwrap();

        let (_, next_cursor) = cursor.remove();

        cursor = next_cursor.unwrap();
    }

    black_box(cursor.pos());
}

fn packeddeque_insert_before_remove(deque: &mut MyDequePacked<usize>) {
    let middle = N / 2;
    let mut cursor = deque.at(middle).unwrap();

    for i in 0..MIDDLE_OPS {
        // Insert before the current node.
        cursor = cursor.insert_before(black_box(i));

        // Return to the newly inserted node.
        cursor = cursor.next().unwrap();

        // Remove it. The returned cursor points at the old current node.
        let (_, next_cursor) = cursor.remove();

        cursor = next_cursor.unwrap();
    }

    black_box(cursor.pos());
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

fn vecdeque_insert_remove(deque: &mut VecDeque<usize>) {
    let middle = N / 2;

    for i in 0..MIDDLE_OPS {
        deque.insert(middle, black_box(i));

        let value = deque.remove(middle).unwrap();

        black_box(value);
    }
}

// ============================================================
// Steady-state churn
//
// Keep length == N.
//
// pop_front()
// push_back()
//
// MyDeque / MyDequeAos:
//     free-list reuse.
//
// VecDeque:
//     existing capacity reuse.
//
// LinkedList / fast-list:
//     remove old element and allocate a replacement.
// ============================================================

fn mydeque_churn(deque: &mut MyDeque<usize>) {
    for i in 0..CHURN_OPS {
        let value = deque.pop_front().unwrap();

        black_box(value);

        let _ = deque.push_back(black_box(i));
    }
}

fn aosdeque_churn(deque: &mut MyDequeAos<usize>) {
    for i in 0..CHURN_OPS {
        let value = deque.pop_front().unwrap();

        black_box(value);

        let _ = deque.push_back(black_box(i));
    }
}

fn packeddeque_churn(deque: &mut MyDequePacked<usize>) {
    for i in 0..CHURN_OPS {
        let value = deque.pop_front().unwrap();

        black_box(value);

        let _ = deque.push_back(black_box(i));
    }
}

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

// ============================================================
// Random positional remove + insert
//
// This deliberately includes POSITION LOOKUP.
//
// MyDeque / MyDequeAos:
//     at(pos)            O(N)
//     remove()           O(1)
//     insert_before()    O(1)
//
// std LinkedList:
//     cursor positioning O(N)
//     remove_current()   O(1)
//     insert_before()    O(1)
//
// fast-list:
//     nth(pos)           O(N)
//     remove(index)      O(1)
//     insert_before()    O(1)
//
// VecDeque:
//     index lookup       O(1)
//     remove(pos)        O(N)
//     insert(pos)        O(N)
// ============================================================

fn mydeque_random_remove_insert(deque: &mut MyDeque<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let cursor = deque.at(pos).unwrap();

        let (_, next_cursor) = cursor.remove();

        match next_cursor {
            Some(cursor) => {
                drop(cursor.insert_before(black_box(i)));
            }

            None => {
                let _ = deque.push_back(black_box(i));
            }
        }
    }

    black_box(deque.len());
}

fn aosdeque_random_remove_insert(deque: &mut MyDequeAos<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let cursor = deque.at(pos).unwrap();

        let (_, next_cursor) = cursor.remove();

        match next_cursor {
            Some(cursor) => {
                drop(cursor.insert_before(black_box(i)));
            }

            None => {
                let _ = deque.push_back(black_box(i));
            }
        }
    }

    black_box(deque.len());
}

fn packeddeque_random_remove_insert(deque: &mut MyDequePacked<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let cursor = deque.at(pos).unwrap();

        let (_, next_cursor) = cursor.remove();

        match next_cursor {
            Some(cursor) => {
                drop(cursor.insert_before(black_box(i)));
            }

            None => {
                let _ = deque.push_back(black_box(i));
            }
        }
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

fn vecdeque_random_remove_insert(deque: &mut VecDeque<usize>, positions: &[usize]) {
    for (i, &pos) in positions.iter().enumerate() {
        let value = deque.remove(pos).unwrap();

        black_box(value);

        deque.insert(pos, black_box(i));
    }

    black_box(deque.len());
}

// ============================================================
// Known-position read + write
//
// Positioning is done once.
//
// This benchmark intentionally includes an opaque per-iteration
// input so the compiler cannot trivially collapse the entire
// loop into one arithmetic expression.
// ============================================================

fn mydeque_cursor_update(deque: &mut MyDeque<usize>) {
    let mut cursor = deque.at(N / 2).unwrap();

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = *cursor.value();

        let new_value = value.wrapping_add(delta);

        *cursor.value_mut() = new_value;

        checksum ^= black_box(new_value);
    }

    black_box(checksum);
}

fn aosdeque_cursor_update(deque: &mut MyDequeAos<usize>) {
    let mut cursor = deque.at(N / 2).unwrap();

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = *cursor.value();

        let new_value = value.wrapping_add(delta);

        *cursor.value_mut() = new_value;

        checksum ^= black_box(new_value);
    }

    black_box(checksum);
}

fn packeddeque_cursor_update(deque: &mut MyDequePacked<usize>) {
    let mut cursor = deque.at(N / 2).unwrap();

    let mut checksum = 0usize;

    for i in 0..CURSOR_UPDATE_OPS {
        let delta = black_box(i.wrapping_mul(0x9e37_79b9));

        let value = *cursor.value();

        let new_value = value.wrapping_add(delta);

        *cursor.value_mut() = new_value;

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

// ============================================================
// 64-byte element type
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

// ============================================================
// 64-byte constructors
// ============================================================

fn make_mydeque_blob() -> MyDeque<Blob64> {
    let mut deque = MyDeque::new();

    for i in 0..LARGE_N {
        deque.push_back(Blob64::new(i));
    }

    deque
}

fn make_aosdeque_blob() -> MyDequeAos<Blob64> {
    let mut deque = MyDequeAos::new();

    for i in 0..LARGE_N {
        deque.push_back(Blob64::new(i));
    }

    deque
}

fn make_packeddeque_blob() -> MyDequePacked<Blob64> {
    let mut deque = MyDequePacked::new();

    for i in 0..LARGE_N {
        deque.push_back(Blob64::new(i));
    }

    deque
}

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

// ============================================================
// 64-byte iteration
// ============================================================

fn mydeque_blob_iter(deque: &MyDeque<Blob64>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
}

fn aosdeque_blob_iter(deque: &MyDequeAos<Blob64>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
}

fn packeddeque_blob_iter(deque: &MyDequePacked<Blob64>) {
    let mut sum = 0usize;

    for value in deque.iter() {
        sum = sum.wrapping_add(value.key());
    }

    black_box(sum);
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

// ============================================================
// 64-byte end operations
// ============================================================

fn mydeque_blob_push_back_pop_front() {
    let mut deque = MyDeque::new();

    for i in 0..LARGE_N {
        let _ = deque.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn aosdeque_blob_push_back_pop_front() {
    let mut deque = MyDequeAos::new();

    for i in 0..LARGE_N {
        let _ = deque.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
}

fn packeddeque_blob_push_back_pop_front() {
    let mut deque = MyDequePacked::new();

    for i in 0..LARGE_N {
        let _ = deque.push_back(black_box(Blob64::new(i)));
    }

    while let Some(value) = deque.pop_front() {
        black_box(value);
    }
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
// Main
// ============================================================

fn main() {
    println!("N = {N}, rounds = {ROUNDS}");
    println!();

    // --------------------------------------------------------
    // End operations
    // --------------------------------------------------------

    let mydeque_push_back_pop_front_time = benchmark(mydeque_push_back_pop_front);

    let aosdeque_push_back_pop_front_time = benchmark(aosdeque_push_back_pop_front);

    let packeddeque_push_back_pop_front_time = benchmark(packeddeque_push_back_pop_front);

    let vecdeque_push_back_pop_front_time = benchmark(vecdeque_push_back_pop_front);

    let linkedlist_push_back_pop_front_time = benchmark(linkedlist_push_back_pop_front);

    let fastlist_push_back_pop_front_time = benchmark(fastlist_push_back_pop_front);

    let mydeque_push_front_pop_back_time = benchmark(mydeque_push_front_pop_back);

    let aosdeque_push_front_pop_back_time = benchmark(aosdeque_push_front_pop_back);

    let packeddeque_push_front_pop_back_time = benchmark(packeddeque_push_front_pop_back);

    let vecdeque_push_front_pop_back_time = benchmark(vecdeque_push_front_pop_back);

    let linkedlist_push_front_pop_back_time = benchmark(linkedlist_push_front_pop_back);

    let fastlist_push_front_pop_back_time = benchmark(fastlist_push_front_pop_back);

    println!("End operations:");

    println!(
        "MyDeque      push_back + pop_front: {:?}",
        mydeque_push_back_pop_front_time
    );

    println!(
        "MyDequeAos   push_back + pop_front: {:?}",
        aosdeque_push_back_pop_front_time
    );

    println!(
        "MyDequePacked push_back + pop_front: {:?}",
        packeddeque_push_back_pop_front_time
    );

    println!(
        "VecDeque     push_back + pop_front: {:?}",
        vecdeque_push_back_pop_front_time
    );

    println!(
        "LinkedList   push_back + pop_front: {:?}",
        linkedlist_push_back_pop_front_time
    );

    println!(
        "FastList     push_back + pop_front: {:?}",
        fastlist_push_back_pop_front_time
    );

    println!();

    println!(
        "MyDeque      push_front + pop_back: {:?}",
        mydeque_push_front_pop_back_time
    );

    println!(
        "MyDequeAos   push_front + pop_back: {:?}",
        aosdeque_push_front_pop_back_time
    );

    println!(
        "MyDequePacked push_front + pop_back: {:?}",
        packeddeque_push_front_pop_back_time
    );

    println!(
        "VecDeque     push_front + pop_back: {:?}",
        vecdeque_push_front_pop_back_time
    );

    println!(
        "LinkedList   push_front + pop_back: {:?}",
        linkedlist_push_front_pop_back_time
    );

    println!(
        "FastList     push_front + pop_back: {:?}",
        fastlist_push_front_pop_back_time
    );

    println!();

    // --------------------------------------------------------
    // Construct reusable containers
    // --------------------------------------------------------

    let mydeque = make_mydeque();
    let aosdeque = make_aosdeque();
    let packeddeque = make_packeddeque();
    let vecdeque = make_vecdeque();
    let linkedlist = make_linkedlist();
    let fastlist = make_fastlist();
    let vec = make_vec();

    // --------------------------------------------------------
    // Iteration
    // --------------------------------------------------------

    let mydeque_iter_time = benchmark(|| mydeque_iter(&mydeque));

    let aosdeque_iter_time = benchmark(|| aosdeque_iter(&aosdeque));

    let packeddeque_iter_time = benchmark(|| packeddeque_iter(&packeddeque));

    let vecdeque_iter_time = benchmark(|| vecdeque_iter(&vecdeque));

    let linkedlist_iter_time = benchmark(|| linkedlist_iter(&linkedlist));

    let fastlist_iter_time = benchmark(|| fastlist_iter(&fastlist));

    let vec_iter_time = benchmark(|| vec_iter(&vec));

    println!("Iteration:");

    println!("MyDeque      iter: {:?}", mydeque_iter_time);

    println!("MyDequeAos   iter: {:?}", aosdeque_iter_time);

    println!("MyDequePacked iter: {:?}", packeddeque_iter_time);

    println!("VecDeque     iter: {:?}", vecdeque_iter_time);

    println!("LinkedList   iter: {:?}", linkedlist_iter_time);

    println!("FastList     iter: {:?}", fastlist_iter_time);

    println!("Vec          iter: {:?}", vec_iter_time);

    println!();

    // --------------------------------------------------------
    // Middle positional access
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();

    let mut aosdeque = make_aosdeque();

    let mut packeddeque = make_packeddeque();

    let vecdeque = make_vecdeque();

    let linkedlist = make_linkedlist();

    let fastlist = make_fastlist();

    let mydeque_at_time = benchmark(|| mydeque_at_middle(&mut mydeque));

    let aosdeque_at_time = benchmark(|| aosdeque_at_middle(&mut aosdeque));

    let packeddeque_at_time = benchmark(|| packeddeque_at_middle(&mut packeddeque));

    let vecdeque_index_time = benchmark(|| vecdeque_index_middle(&vecdeque));

    let linkedlist_at_time = benchmark(|| linkedlist_at_middle(&linkedlist));

    let fastlist_at_time = benchmark(|| fastlist_at_middle(&fastlist));

    println!("Middle access ({LOOKUPS} lookups):");

    println!("MyDeque      at():     {:?}", mydeque_at_time);

    println!("MyDequeAos   at():     {:?}", aosdeque_at_time);

    println!("MyDequePacked at():     {:?}", packeddeque_at_time);

    println!("VecDeque     index:    {:?}", vecdeque_index_time);

    println!("LinkedList   iter nth: {:?}", linkedlist_at_time);

    println!("FastList     nth:      {:?}", fastlist_at_time);

    println!();

    // --------------------------------------------------------
    // Known-position local insertion/removal
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();

    let mut aosdeque = make_aosdeque();

    let mut packeddeque = make_packeddeque();

    let mut vecdeque = make_vecdeque();

    let mut linkedlist = make_linkedlist();

    let mut fastlist = make_fastlist();

    let mydeque_insert_remove_time = benchmark(|| mydeque_insert_before_remove(&mut mydeque));

    let aosdeque_insert_remove_time = benchmark(|| aosdeque_insert_before_remove(&mut aosdeque));

    let packeddeque_insert_remove_time =
        benchmark(|| packeddeque_insert_before_remove(&mut packeddeque));

    let linkedlist_insert_remove_time =
        benchmark(|| linkedlist_insert_before_remove(&mut linkedlist));

    let fastlist_insert_remove_time = benchmark(|| fastlist_insert_before_remove(&mut fastlist));

    let vecdeque_insert_remove_time = benchmark(|| vecdeque_insert_remove(&mut vecdeque));

    println!("Middle insert + remove ({MIDDLE_OPS} operations):");

    println!(
        "MyDeque      cursor insert_before + remove: {:?}",
        mydeque_insert_remove_time
    );

    println!(
        "MyDequeAos   cursor insert_before + remove: {:?}",
        aosdeque_insert_remove_time
    );

    println!(
        "MyDequePacked cursor insert_before + remove: {:?}",
        packeddeque_insert_remove_time
    );

    println!(
        "LinkedList   cursor insert_before + remove: {:?}",
        linkedlist_insert_remove_time
    );

    println!(
        "FastList     index  insert_before + remove: {:?}",
        fastlist_insert_remove_time
    );

    println!(
        "VecDeque     index  insert + remove:          {:?}",
        vecdeque_insert_remove_time
    );

    println!();

    // --------------------------------------------------------
    // Steady-state churn
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();

    let mut aosdeque = make_aosdeque();

    let mut packeddeque = make_packeddeque();

    let mut vecdeque = make_vecdeque();

    let mut linkedlist = make_linkedlist();

    let mut fastlist = make_fastlist();

    let mydeque_churn_time = benchmark(|| mydeque_churn(&mut mydeque));

    let aosdeque_churn_time = benchmark(|| aosdeque_churn(&mut aosdeque));

    let packeddeque_churn_time = benchmark(|| packeddeque_churn(&mut packeddeque));

    let vecdeque_churn_time = benchmark(|| vecdeque_churn(&mut vecdeque));

    let linkedlist_churn_time = benchmark(|| linkedlist_churn(&mut linkedlist));

    let fastlist_churn_time = benchmark(|| fastlist_churn(&mut fastlist));

    println!("Steady-state churn ({CHURN_OPS} pop_front + push_back):");

    println!("MyDeque      : {:?}", mydeque_churn_time);

    println!("MyDequeAos   : {:?}", aosdeque_churn_time);

    println!("MyDequePacked : {:?}", packeddeque_churn_time);

    println!("VecDeque     : {:?}", vecdeque_churn_time);

    println!("LinkedList   : {:?}", linkedlist_churn_time);

    println!("FastList     : {:?}", fastlist_churn_time);

    println!();

    // --------------------------------------------------------
    // Random positional remove + insert
    // --------------------------------------------------------

    let positions = make_random_positions(N, RANDOM_OPS);

    let mut mydeque = make_mydeque();

    let mut aosdeque = make_aosdeque();

    let mut packeddeque = make_packeddeque();

    let mut vecdeque = make_vecdeque();

    let mut linkedlist = make_linkedlist();

    let mut fastlist = make_fastlist();

    let mydeque_random_time = benchmark(|| mydeque_random_remove_insert(&mut mydeque, &positions));

    let aosdeque_random_time =
        benchmark(|| aosdeque_random_remove_insert(&mut aosdeque, &positions));

    let packeddeque_random_time =
        benchmark(|| packeddeque_random_remove_insert(&mut packeddeque, &positions));

    let linkedlist_random_time =
        benchmark(|| linkedlist_random_remove_insert(&mut linkedlist, &positions));

    let fastlist_random_time =
        benchmark(|| fastlist_random_remove_insert(&mut fastlist, &positions));

    let vecdeque_random_time =
        benchmark(|| vecdeque_random_remove_insert(&mut vecdeque, &positions));

    println!("Random positional remove + insert ({RANDOM_OPS} operations):");

    println!("MyDeque      : {:?}", mydeque_random_time);

    println!("MyDequeAos   : {:?}", aosdeque_random_time);

    println!("MyDequePacked : {:?}", packeddeque_random_time);

    println!("LinkedList   : {:?}", linkedlist_random_time);

    println!("FastList     : {:?}", fastlist_random_time);

    println!("VecDeque     : {:?}", vecdeque_random_time);

    println!();

    // --------------------------------------------------------
    // Known-position read + write
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();

    let mut aosdeque = make_aosdeque();

    let mut vecdeque = make_vecdeque();

    let mut linkedlist = make_linkedlist();

    let mut fastlist = make_fastlist();

    let mut packeddeque = make_packeddeque();

    let mydeque_update_time = benchmark(|| mydeque_cursor_update(&mut mydeque));

    let aosdeque_update_time = benchmark(|| aosdeque_cursor_update(&mut aosdeque));

    let packeddeque_update_time = benchmark(|| packeddeque_cursor_update(&mut packeddeque));

    let linkedlist_update_time = benchmark(|| linkedlist_cursor_update(&mut linkedlist));

    let fastlist_update_time = benchmark(|| fastlist_index_update(&mut fastlist));

    let vecdeque_update_time = benchmark(|| vecdeque_index_update(&mut vecdeque));

    println!("Known-position read + write ({CURSOR_UPDATE_OPS} operations):");

    println!("MyDeque      : {:?}", mydeque_update_time);

    println!("MyDequeAos   : {:?}", aosdeque_update_time);

    println!("MyDequePacked : {:?}", packeddeque_update_time);

    println!("LinkedList   : {:?}", linkedlist_update_time);

    println!("FastList     : {:?}", fastlist_update_time);

    println!("VecDeque     : {:?}", vecdeque_update_time);

    println!();

    // --------------------------------------------------------
    // 64-byte T: iteration
    // --------------------------------------------------------

    let mydeque_blob = make_mydeque_blob();

    let aosdeque_blob = make_aosdeque_blob();

    let packeddeque_blob = make_packeddeque_blob();

    let vecdeque_blob = make_vecdeque_blob();

    let linkedlist_blob = make_linkedlist_blob();

    let fastlist_blob = make_fastlist_blob();

    let vec_blob = make_vec_blob();

    let mydeque_blob_iter_time = benchmark(|| mydeque_blob_iter(&mydeque_blob));

    let aosdeque_blob_iter_time = benchmark(|| aosdeque_blob_iter(&aosdeque_blob));

    let packeddeque_blob_iter_time = benchmark(|| packeddeque_blob_iter(&packeddeque_blob));

    let vecdeque_blob_iter_time = benchmark(|| vecdeque_blob_iter(&vecdeque_blob));

    let linkedlist_blob_iter_time = benchmark(|| linkedlist_blob_iter(&linkedlist_blob));

    let fastlist_blob_iter_time = benchmark(|| fastlist_blob_iter(&fastlist_blob));

    let vec_blob_iter_time = benchmark(|| vec_blob_iter(&vec_blob));

    println!("Iteration with 64-byte T (N = {LARGE_N}):");

    println!("MyDeque      : {:?}", mydeque_blob_iter_time);

    println!("MyDequeAos   : {:?}", aosdeque_blob_iter_time);

    println!("MyDequePacked : {:?}", packeddeque_blob_iter_time);

    println!("VecDeque     : {:?}", vecdeque_blob_iter_time);

    println!("LinkedList   : {:?}", linkedlist_blob_iter_time);

    println!("FastList     : {:?}", fastlist_blob_iter_time);

    println!("Vec          : {:?}", vec_blob_iter_time);

    println!();

    // --------------------------------------------------------
    // 64-byte T: end operations
    // --------------------------------------------------------

    let mydeque_blob_end_time = benchmark(mydeque_blob_push_back_pop_front);

    let aosdeque_blob_end_time = benchmark(aosdeque_blob_push_back_pop_front);

    let packeddeque_blob_end_time = benchmark(packeddeque_blob_push_back_pop_front);

    let vecdeque_blob_end_time = benchmark(vecdeque_blob_push_back_pop_front);

    let linkedlist_blob_end_time = benchmark(linkedlist_blob_push_back_pop_front);

    let fastlist_blob_end_time = benchmark(fastlist_blob_push_back_pop_front);

    println!("End operations with 64-byte T (N = {LARGE_N}):");

    println!("MyDeque      : {:?}", mydeque_blob_end_time);

    println!("MyDequeAos   : {:?}", aosdeque_blob_end_time);

    println!("MyDequePacked : {:?}", packeddeque_blob_end_time);

    println!("VecDeque     : {:?}", vecdeque_blob_end_time);

    println!("LinkedList   : {:?}", linkedlist_blob_end_time);

    println!("FastList     : {:?}", fastlist_blob_end_time);

    println!();
}
