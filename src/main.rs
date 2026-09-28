mod my_deque;

use my_deque::MyDeque;

use std::{
    collections::VecDeque,
    hint::black_box,
    time::{Duration, Instant},
};

const N: usize = 1_000_000;
const ROUNDS: usize = 10;
const LOOKUPS: usize = 100;
const MIDDLE_OPS: usize = 1_000;

fn benchmark<F: FnMut()>(mut f: F) -> Duration {
    // Warmup
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
// End operations
// ============================================================

fn mydeque_push_back_pop_front() {
    let mut deque = MyDeque::new();

    for i in 0..N {
        deque.push_back(black_box(i));
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

fn mydeque_push_front_pop_back() {
    let mut deque = MyDeque::new();

    for i in 0..N {
        deque.push_front(black_box(i));
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

// ============================================================
// Pure iteration
// ============================================================

fn make_mydeque() -> MyDeque<usize> {
    let mut deque = MyDeque::new();

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

fn make_vec() -> Vec<usize> {
    (0..N).collect()
}

fn mydeque_iter(deque: &MyDeque<usize>) {
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

fn vec_iter(vec: &[usize]) {
    let mut sum = 0usize;

    for value in vec {
        sum = sum.wrapping_add(*value);
    }

    black_box(sum);
}

// ============================================================
// Random-ish positional access
// ============================================================

fn mydeque_at_middle(deque: &mut MyDeque<usize>) {
    let mut sum = 0usize;
    let middle = N / 2;

    for _ in 0..LOOKUPS {
        let cursor = deque.at(middle).unwrap();
        sum = sum.wrapping_add(*cursor.value());
    }

    black_box(sum);
}

fn vecdeque_index_middle(deque: &VecDeque<usize>) {
    let mut sum = 0usize;
    let middle = N / 2;

    for _ in 0..LOOKUPS {
        sum = sum.wrapping_add(deque[middle]);
    }

    black_box(sum);
}

// ============================================================
// Known-cursor insertion/removal
//
// We deliberately keep the cursor at roughly the same logical
// position and repeatedly insert before it, then remove the
// inserted node.
//
// This measures the linked structure's O(1) local mutation,
// without repeatedly calling at().
// ============================================================

fn mydeque_insert_before_remove(deque: &mut MyDeque<usize>) {
    let middle = N / 2;
    let mut cursor = deque.at(middle).unwrap();

    for i in 0..MIDDLE_OPS {
        let inserted = cursor.insert_before(black_box(i));
        let (_, next_cursor) = inserted.remove();

        cursor = next_cursor.unwrap();
    }

    black_box(cursor.pos());
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
// Main
// ============================================================

fn main() {
    println!("N = {N}, rounds = {ROUNDS}");
    println!();

    let mydeque_push_back_pop_front_time = benchmark(mydeque_push_back_pop_front);

    let vecdeque_push_back_pop_front_time = benchmark(vecdeque_push_back_pop_front);

    let mydeque_push_front_pop_back_time = benchmark(mydeque_push_front_pop_back);

    let vecdeque_push_front_pop_back_time = benchmark(vecdeque_push_front_pop_back);

    println!(
        "MyDeque  push_back + pop_front: {:?}",
        mydeque_push_back_pop_front_time
    );
    println!(
        "VecDeque push_back + pop_front: {:?}",
        vecdeque_push_back_pop_front_time
    );
    println!();

    println!(
        "MyDeque  push_front + pop_back: {:?}",
        mydeque_push_front_pop_back_time
    );
    println!(
        "VecDeque push_front + pop_back: {:?}",
        vecdeque_push_front_pop_back_time
    );
    println!();

    // --------------------------------------------------------
    // Iteration
    // --------------------------------------------------------

    let mydeque = make_mydeque();
    let vecdeque = make_vecdeque();
    let vec = make_vec();

    let mydeque_iter_time = benchmark(|| mydeque_iter(&mydeque));

    let vecdeque_iter_time = benchmark(|| vecdeque_iter(&vecdeque));

    let vec_iter_time = benchmark(|| vec_iter(&vec));

    println!("Iteration:");
    println!("MyDeque  iter: {:?}", mydeque_iter_time);
    println!("VecDeque iter: {:?}", vecdeque_iter_time);
    println!("Vec      iter: {:?}", vec_iter_time);
    println!();

    // --------------------------------------------------------
    // Positional access
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();
    let vecdeque = make_vecdeque();

    let mydeque_at_time = benchmark(|| mydeque_at_middle(&mut mydeque));

    let vecdeque_index_time = benchmark(|| vecdeque_index_middle(&vecdeque));

    println!("Middle access ({LOOKUPS} lookups):");
    println!("MyDeque  at():     {:?}", mydeque_at_time);
    println!("VecDeque index:    {:?}", vecdeque_index_time);
    println!();

    // --------------------------------------------------------
    // Local insertion/removal
    // --------------------------------------------------------

    let mut mydeque = make_mydeque();
    let mut vecdeque = make_vecdeque();

    let mydeque_insert_remove_time = benchmark(|| mydeque_insert_before_remove(&mut mydeque));

    let vecdeque_insert_remove_time = benchmark(|| vecdeque_insert_remove(&mut vecdeque));

    println!("Middle insert + remove ({MIDDLE_OPS} operations):");
    println!(
        "MyDeque  cursor insert_before + remove: {:?}",
        mydeque_insert_remove_time
    );
    println!(
        "VecDeque index insert + remove:          {:?}",
        vecdeque_insert_remove_time
    );
}
