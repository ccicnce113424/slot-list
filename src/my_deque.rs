use std::mem::MaybeUninit;

pub struct MyDeque<T> {
    data: Vec<MaybeUninit<T>>,
    prev: Vec<Option<usize>>,
    next: Vec<Option<usize>>,
    head: Option<usize>,
    tail: Option<usize>,
    free_head: Option<usize>,
    len: usize,
}

pub struct Cursor<'a, T> {
    deque: &'a mut MyDeque<T>,
    index: usize,
    pos: usize,
}

pub struct Iter<'a, T> {
    deque: &'a MyDeque<T>,
    index: Option<usize>,
}

impl<T> MyDeque<T> {
    pub fn new() -> Self {
        MyDeque {
            data: Vec::new(),
            prev: Vec::new(),
            next: Vec::new(),
            head: None,
            tail: None,
            free_head: None,
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn data(&self, index: usize) -> &MaybeUninit<T> {
        unsafe { self.data.get_unchecked(index) }
    }
    fn data_mut(&mut self, index: usize) -> &mut MaybeUninit<T> {
        unsafe { self.data.get_unchecked_mut(index) }
    }
    fn prev(&self, index: usize) -> &Option<usize> {
        unsafe { self.prev.get_unchecked(index) }
    }
    fn prev_mut(&mut self, index: usize) -> &mut Option<usize> {
        unsafe { self.prev.get_unchecked_mut(index) }
    }
    fn next(&self, index: usize) -> &Option<usize> {
        unsafe { self.next.get_unchecked(index) }
    }
    fn next_mut(&mut self, index: usize) -> &mut Option<usize> {
        unsafe { self.next.get_unchecked_mut(index) }
    }

    fn alloc_node(&mut self, value: T) -> usize {
        self.len += 1;
        let index = if let Some(free_index) = self.free_head {
            self.free_head = *self.next(free_index);
            free_index
        } else {
            self.data.push(MaybeUninit::uninit());
            self.prev.push(None);
            self.next.push(None);
            self.data.len() - 1
        };
        self.data_mut(index).write(value);
        index
    }

    fn free_node(&mut self, index: usize) -> T {
        self.len -= 1;
        *self.next_mut(index) = self.free_head;
        self.free_head = Some(index);
        unsafe { self.data_mut(index).assume_init_read() }
    }

    fn insert_link(&mut self, pos: usize, prev: Option<usize>, next: Option<usize>) {
        if let Some(prev_index) = prev {
            *self.next_mut(prev_index) = Some(pos);
            *self.prev_mut(pos) = Some(prev_index);
        } else {
            self.head = Some(pos);
            *self.prev_mut(pos) = None;
        }
        if let Some(next_index) = next {
            *self.prev_mut(next_index) = Some(pos);
            *self.next_mut(pos) = Some(next_index);
        } else {
            self.tail = Some(pos);
            *self.next_mut(pos) = None;
        }
    }

    fn remove_link(&mut self, prev: Option<usize>, next: Option<usize>) {
        if let Some(prev_index) = prev {
            *self.next_mut(prev_index) = next;
        } else {
            self.head = next;
        }
        if let Some(next_index) = next {
            *self.prev_mut(next_index) = prev;
        } else {
            self.tail = prev;
        }
    }

    fn seek_index(&self, index: usize, offset: isize) -> Option<usize> {
        if offset < 0 {
            self.move_backward(index, (-offset) as usize)
        } else {
            self.move_forward(index, offset as usize)
        }
    }

    fn move_forward(&self, index: usize, steps: usize) -> Option<usize> {
        let mut current_index = index;
        for _ in 0..steps {
            let next = *self.next(current_index);
            current_index = next?;
        }
        Some(current_index)
    }

    fn move_backward(&self, index: usize, steps: usize) -> Option<usize> {
        let mut current_index = index;
        for _ in 0..steps {
            let prev = *self.prev(current_index);
            current_index = prev?;
        }
        Some(current_index)
    }

    pub fn push_front(&mut self, value: T) -> Cursor<'_, T> {
        let new_index = self.alloc_node(value);
        self.insert_link(new_index, None, self.head);
        Cursor {
            deque: self,
            index: new_index,
            pos: 0,
        }
    }

    pub fn push_back(&mut self, value: T) -> Cursor<'_, T> {
        let new_index = self.alloc_node(value);
        self.insert_link(new_index, self.tail, None);
        Cursor {
            pos: self.len - 1,
            deque: self,
            index: new_index,
        }
    }
    pub fn pop_front(&mut self) -> Option<T> {
        let head_index = self.head?;
        self.remove_link(None, *self.next(head_index));
        Some(self.free_node(head_index))
    }

    pub fn pop_back(&mut self) -> Option<T> {
        let tail_index = self.tail?;
        self.remove_link(*self.prev(tail_index), None);
        Some(self.free_node(tail_index))
    }
    pub fn front(&mut self) -> Option<Cursor<'_, T>> {
        self.head.map(|head_index| Cursor {
            deque: self,
            index: head_index,
            pos: 0,
        })
    }
    pub fn back(&mut self) -> Option<Cursor<'_, T>> {
        self.tail.map(|tail_index| Cursor {
            pos: self.len - 1,
            deque: self,
            index: tail_index,
        })
    }
    pub fn at(&mut self, pos: usize) -> Option<Cursor<'_, T>> {
        let cursor = if pos < self.len / 2 {
            self.front()?
        } else {
            self.back()?
        };
        cursor.seek(pos)
    }
    pub fn clear(&mut self) {
        while self.pop_front().is_some() {}
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            deque: self,
            index: self.head,
        }
    }
}

impl<T> Drop for MyDeque<T> {
    fn drop(&mut self) {
        self.clear();
    }
}

impl<T> Cursor<'_, T> {
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn value(&self) -> &T {
        unsafe { self.deque.data(self.index).assume_init_ref() }
    }
    pub fn value_mut(&mut self) -> &mut T {
        unsafe { self.deque.data_mut(self.index).assume_init_mut() }
    }
    pub fn prev(self) -> Option<Self> {
        self.deque.prev(self.index).map(|prev_index| Cursor {
            deque: self.deque,
            index: prev_index,
            pos: self.pos - 1,
        })
    }
    pub fn next(self) -> Option<Self> {
        self.deque.next(self.index).map(|next_index| Cursor {
            deque: self.deque,
            index: next_index,
            pos: self.pos + 1,
        })
    }
    pub fn move_steps(self, offset: isize) -> Option<Self> {
        self.deque
            .seek_index(self.index, offset)
            .map(|index| Cursor {
                deque: self.deque,
                index,
                pos: (self.pos as isize + offset) as usize,
            })
    }
    pub fn seek(self, pos: usize) -> Option<Self> {
        let diff = pos as isize - self.pos as isize;
        self.move_steps(diff)
    }
    pub fn is_head(&self) -> bool {
        self.deque.head == Some(self.index)
    }
    pub fn is_tail(&self) -> bool {
        self.deque.tail == Some(self.index)
    }
    pub fn insert_before(self, value: T) -> Self {
        let new_index = self.deque.alloc_node(value);
        let prev_index = *self.deque.prev(self.index);
        self.deque
            .insert_link(new_index, prev_index, Some(self.index));
        Cursor {
            deque: self.deque,
            index: new_index,
            pos: self.pos,
        }
    }
    pub fn insert_after(self, value: T) -> Self {
        let new_index = self.deque.alloc_node(value);
        let next_index = *self.deque.next(self.index);
        self.deque
            .insert_link(new_index, Some(self.index), next_index);
        Cursor {
            deque: self.deque,
            index: new_index,
            pos: self.pos + 1,
        }
    }
    pub fn remove(self) -> (T, Option<Self>) {
        let prev_index = *self.deque.prev(self.index);
        let next_index = *self.deque.next(self.index);
        self.deque.remove_link(prev_index, next_index);
        (
            self.deque.free_node(self.index),
            next_index.map(|index| Cursor {
                deque: self.deque,
                index,
                pos: self.pos,
            }),
        )
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<Self::Item> {
        let index = self.index?;
        let item = self.deque.data(index);
        self.index = *self.deque.next(index);
        unsafe { Some(item.assume_init_ref()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 检查 MyDeque 的内部不变量。
    fn assert_valid<T>(deque: &MyDeque<T>) {
        // 三个 Vec 必须保持相同长度。
        assert_eq!(deque.data.len(), deque.prev.len());
        assert_eq!(deque.data.len(), deque.next.len());

        // 空 / 非空状态必须与 head / tail 一致。
        if deque.len == 0 {
            assert!(deque.head.is_none());
            assert!(deque.tail.is_none());
        } else {
            assert!(deque.head.is_some());
            assert!(deque.tail.is_some());
        }

        // 只要有 head，就必须没有前驱。
        if let Some(head) = deque.head {
            assert_eq!(deque.prev[head], None);
        }

        // 只要有 tail，就必须没有后继。
        if let Some(tail) = deque.tail {
            assert_eq!(deque.next[tail], None);
        }

        // 沿 next 从 head 遍历：
        // 1. 每个节点都必须是 live node
        // 2. prev / next 必须互相一致
        // 3. 遍历出来的 live node 数量必须等于 len
        let mut current = deque.head;
        let mut previous = None;
        let mut live_count = 0;

        while let Some(index) = current {
            assert!(index < deque.data.len());
            // assert!(deque.data[index].is_some());

            // 当前节点的 prev 必须就是刚才那个节点。
            assert_eq!(deque.prev[index], previous);

            // 如果有 next，那么 next 的 prev 必须指回来。
            if let Some(next) = deque.next[index] {
                assert!(next < deque.data.len());
                assert_eq!(deque.prev[next], Some(index));
            }

            previous = Some(index);
            current = deque.next[index];

            live_count += 1;

            // 如果出现环，这里可以尽早失败。
            assert!(
                live_count <= deque.len,
                "next links contain a cycle or len is incorrect"
            );
        }

        assert_eq!(live_count, deque.len);
        assert_eq!(previous, deque.tail);

        // 检查 free list。
        //
        // free list 中的节点必须：
        // 1. 是合法 index
        // 2. data == None
        // 3. 不得与 live list 重叠
        let mut free_current = deque.free_head;
        let mut free_count = 0;

        while let Some(index) = free_current {
            assert!(index < deque.data.len());
            // assert!(deque.data[index].is_none());

            // free node 的 next 被拿来串 free list。
            free_current = deque.next[index];

            free_count += 1;

            // 防止 free list 自己形成环。
            assert!(free_count <= deque.data.len(), "free list contains a cycle");
        }

        // live + free 应该正好覆盖所有 slot。
        assert_eq!(live_count + free_count, deque.data.len());
    }

    #[test]
    fn new() {
        let deque = MyDeque::<i32>::new();

        assert_eq!(deque.len(), 0);
        assert!(deque.is_empty());

        assert_valid(&deque);
    }

    #[test]
    fn push_back() {
        let mut deque = MyDeque::new();

        deque.push_back(1);
        assert_valid(&deque);

        deque.push_back(2);
        assert_valid(&deque);

        deque.push_back(3);
        assert_valid(&deque);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn push_front() {
        let mut deque = MyDeque::new();

        deque.push_front(3);
        assert_valid(&deque);

        deque.push_front(2);
        assert_valid(&deque);

        deque.push_front(1);
        assert_valid(&deque);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn mixed_push() {
        let mut deque = MyDeque::new();

        deque.push_back(2);
        assert_valid(&deque);

        deque.push_front(1);
        assert_valid(&deque);

        deque.push_back(3);
        assert_valid(&deque);

        deque.push_front(0);
        assert_valid(&deque);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn pop_front() {
        let mut deque = MyDeque::new();

        for i in 0..5 {
            deque.push_back(i);
        }

        assert_valid(&deque);

        assert_eq!(deque.pop_front(), Some(0));
        assert_valid(&deque);

        assert_eq!(deque.pop_front(), Some(1));
        assert_valid(&deque);

        assert_eq!(deque.pop_front(), Some(2));
        assert_valid(&deque);

        assert_eq!(deque.pop_front(), Some(3));
        assert_valid(&deque);

        assert_eq!(deque.pop_front(), Some(4));
        assert_valid(&deque);

        assert_eq!(deque.pop_front(), None);
        assert_valid(&deque);

        assert!(deque.is_empty());
    }

    #[test]
    fn pop_back() {
        let mut deque = MyDeque::new();

        for i in 0..5 {
            deque.push_back(i);
        }

        assert_valid(&deque);

        assert_eq!(deque.pop_back(), Some(4));
        assert_valid(&deque);

        assert_eq!(deque.pop_back(), Some(3));
        assert_valid(&deque);

        assert_eq!(deque.pop_back(), Some(2));
        assert_valid(&deque);

        assert_eq!(deque.pop_back(), Some(1));
        assert_valid(&deque);

        assert_eq!(deque.pop_back(), Some(0));
        assert_valid(&deque);

        assert_eq!(deque.pop_back(), None);
        assert_valid(&deque);

        assert!(deque.is_empty());
    }

    #[test]
    fn cursor_navigation() {
        let mut deque = MyDeque::new();

        for i in 0..5 {
            deque.push_back(i);
        }

        assert_valid(&deque);

        let cursor = deque.front().unwrap();

        assert_eq!(cursor.pos(), 0);
        assert_eq!(*cursor.value(), 0);
        assert!(cursor.is_head());
        assert!(!cursor.is_tail());

        let cursor = cursor.next().unwrap();
        assert_eq!(cursor.pos(), 1);
        assert_eq!(*cursor.value(), 1);

        let cursor = cursor.next().unwrap();
        assert_eq!(cursor.pos(), 2);
        assert_eq!(*cursor.value(), 2);

        let cursor = cursor.prev().unwrap();
        assert_eq!(cursor.pos(), 1);
        assert_eq!(*cursor.value(), 1);

        let cursor = deque.back().unwrap();

        assert_eq!(cursor.pos(), 4);
        assert_eq!(*cursor.value(), 4);
        assert!(!cursor.is_head());
        assert!(cursor.is_tail());

        assert_valid(&deque);
    }

    #[test]
    fn cursor_seek() {
        let mut deque = MyDeque::new();

        for i in 0..10 {
            deque.push_back(i);
        }

        let cursor = deque.at(7).unwrap();

        assert_eq!(cursor.pos(), 7);
        assert_eq!(*cursor.value(), 7);

        let cursor = cursor.seek(2).unwrap();

        assert_eq!(cursor.pos(), 2);
        assert_eq!(*cursor.value(), 2);

        let cursor = cursor.seek(9).unwrap();

        assert_eq!(cursor.pos(), 9);
        assert_eq!(*cursor.value(), 9);

        // seek 不能越过容器边界。
        assert!(cursor.seek(10).is_none());

        assert_valid(&deque);
    }

    #[test]
    fn at_uses_both_directions() {
        let mut deque = MyDeque::new();

        for i in 0..10 {
            deque.push_back(i);
        }

        for pos in 0..10 {
            let cursor = deque.at(pos).unwrap();

            assert_eq!(cursor.pos(), pos);
            assert_eq!(*cursor.value(), pos);
        }

        assert!(deque.at(10).is_none());
        assert_valid(&deque);
    }

    #[test]
    fn cursor_mutate() {
        let mut deque = MyDeque::new();

        deque.push_back(1);
        deque.push_back(2);
        deque.push_back(3);

        let mut cursor = deque.at(1).unwrap();

        *cursor.value_mut() = 42;

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 42, 3]);

        assert_valid(&deque);
    }

    #[test]
    fn insert_before() {
        let mut deque = MyDeque::new();

        deque.push_back(1);
        deque.push_back(3);

        let cursor = deque.at(1).unwrap();
        let cursor = cursor.insert_before(2);

        assert_eq!(cursor.pos(), 1);
        assert_eq!(*cursor.value(), 2);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);

        assert_eq!(deque.len(), 3);
        assert_valid(&deque);
    }

    #[test]
    fn insert_after() {
        let mut deque = MyDeque::new();

        deque.push_back(1);
        deque.push_back(3);

        let cursor = deque.at(0).unwrap();
        let cursor = cursor.insert_after(2);

        assert_eq!(cursor.pos(), 1);
        assert_eq!(*cursor.value(), 2);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3]);

        assert_eq!(deque.len(), 3);
        assert_valid(&deque);
    }

    #[test]
    fn insert_at_edges() {
        let mut deque = MyDeque::new();

        deque.push_back(2);

        let cursor = deque.front().unwrap();
        let cursor = cursor.insert_before(1);

        assert!(cursor.is_head());

        let cursor = cursor.insert_after(1);
        assert!(!cursor.is_head());

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 1, 2]);

        assert_valid(&deque);
    }

    #[test]
    fn remove_middle() {
        let mut deque = MyDeque::new();

        for i in 0..5 {
            deque.push_back(i);
        }

        let cursor = deque.at(2).unwrap();
        let (value, cursor) = cursor.remove();

        assert_eq!(value, 2);

        let cursor = cursor.unwrap();
        assert_eq!(cursor.pos(), 2);
        assert_eq!(*cursor.value(), 3);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![0, 1, 3, 4]);

        assert_eq!(deque.len(), 4);
        assert_valid(&deque);
    }

    #[test]
    fn remove_head() {
        let mut deque = MyDeque::new();

        for i in 0..3 {
            deque.push_back(i);
        }

        let (value, cursor) = deque.front().unwrap().remove();

        assert_eq!(value, 0);

        let cursor = cursor.unwrap();
        assert_eq!(cursor.pos(), 0);
        assert_eq!(*cursor.value(), 1);

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![1, 2]);

        assert_valid(&deque);
    }

    #[test]
    fn remove_tail() {
        let mut deque = MyDeque::new();

        for i in 0..3 {
            deque.push_back(i);
        }

        let (value, cursor) = deque.back().unwrap().remove();

        assert_eq!(value, 2);
        assert!(cursor.is_none());

        assert_eq!(deque.iter().copied().collect::<Vec<_>>(), vec![0, 1]);

        assert!(deque.back().unwrap().is_tail());
        assert_valid(&deque);
    }

    #[test]
    fn remove_only_element() {
        let mut deque = MyDeque::new();

        deque.push_back(123);

        let (value, cursor) = deque.front().unwrap().remove();

        assert_eq!(value, 123);
        assert!(cursor.is_none());

        assert!(deque.is_empty());
        assert!(deque.front().is_none());
        assert!(deque.back().is_none());

        assert_valid(&deque);
    }

    #[test]
    fn iterator() {
        let mut deque = MyDeque::new();

        for i in 0..5 {
            deque.push_back(i);
        }

        let values: Vec<_> = deque.iter().copied().collect();

        assert_eq!(values, vec![0, 1, 2, 3, 4]);
        assert_valid(&deque);
    }

    #[test]
    fn iterator_empty() {
        let deque = MyDeque::<i32>::new();

        assert_eq!(deque.iter().next(), None);
        assert_valid(&deque);
    }

    #[test]
    fn clear() {
        let mut deque = MyDeque::new();

        for i in 0..100 {
            deque.push_back(i);
        }

        deque.clear();

        assert!(deque.is_empty());
        assert_eq!(deque.len(), 0);
        assert!(deque.front().is_none());
        assert!(deque.back().is_none());
        assert_eq!(deque.iter().next(), None);

        assert_valid(&deque);
    }

    #[test]
    fn free_list_reuses_node() {
        let mut deque = MyDeque::new();

        deque.push_back(1);
        deque.push_back(2);
        deque.push_back(3);

        let freed_index = deque.at(1).unwrap().index;

        deque.at(1).unwrap().remove();

        assert_valid(&deque);

        deque.push_back(4);

        let reused_index = deque.back().unwrap().index;

        assert_eq!(reused_index, freed_index);
        assert_valid(&deque);
    }

    #[test]
    fn free_list_reuses_multiple_nodes() {
        let mut deque = MyDeque::new();

        deque.push_back(0);
        deque.push_back(1);
        deque.push_back(2);
        deque.push_back(3);

        let index_1 = deque.at(1).unwrap().index;
        let index_2 = deque.at(2).unwrap().index;

        deque.at(1).unwrap().remove();
        assert_valid(&deque);

        deque.at(1).unwrap().remove();
        assert_valid(&deque);

        // free list 是 LIFO，因此应该先复用最后释放的 index。
        deque.push_back(10);
        assert_eq!(deque.back().unwrap().index, index_2);
        assert_valid(&deque);

        deque.push_back(11);
        assert_eq!(deque.back().unwrap().index, index_1);
        assert_valid(&deque);
    }

    // 用于验证 T 的析构次数。
    struct DropTracker {
        drops: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl DropTracker {
        fn new(drops: &std::rc::Rc<std::cell::Cell<usize>>) -> Self {
            Self {
                drops: std::rc::Rc::clone(drops),
            }
        }
    }

    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn drop_deque_drops_all_live_values() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));

        {
            let mut deque = MyDeque::new();

            for _ in 0..100 {
                deque.push_back(DropTracker::new(&drops));
            }

            assert_eq!(drops.get(), 0);
            assert_eq!(deque.len(), 100);
        }

        // MyDeque::drop -> clear -> pop_front -> free_node
        // 所有 live value 都应该恰好析构一次。
        assert_eq!(drops.get(), 100);
    }

    #[test]
    fn remove_transfers_ownership_without_double_drop() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut deque = MyDeque::new();

        deque.push_back(DropTracker::new(&drops));
        deque.push_back(DropTracker::new(&drops));
        deque.push_back(DropTracker::new(&drops));

        let (value, cursor) = deque.front().unwrap().remove();

        // remove() 只是把 T 移出来，还没有 drop 它。
        assert_eq!(drops.get(), 0);
        assert!(cursor.is_some());
        assert_eq!(deque.len(), 2);

        drop(value);

        // 只有被移出来的这个 T 被析构。
        assert_eq!(drops.get(), 1);

        drop(deque);

        // 剩余两个 live node 再各析构一次。
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn clear_drops_all_live_values() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut deque = MyDeque::new();

        for _ in 0..100 {
            deque.push_back(DropTracker::new(&drops));
        }

        deque.clear();

        assert_eq!(drops.get(), 100);
        assert!(deque.is_empty());

        // clear 后再次 drop 不应该产生额外析构。
        drop(deque);

        assert_eq!(drops.get(), 100);
    }

    #[test]
    fn remove_then_reuse_slot_drops_each_value_once() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut deque = MyDeque::new();

        deque.push_back(DropTracker::new(&drops));

        // 移除第一个值，slot 进入 free list。
        let (value, _) = deque.front().unwrap().remove();

        assert_eq!(drops.get(), 0);
        assert_eq!(deque.len(), 0);

        // 此时 free slot 会被重新使用。
        deque.push_back(DropTracker::new(&drops));

        // 旧 value 尚未 drop，新 value 也刚刚写入。
        assert_eq!(drops.get(), 0);

        // drop 旧 value。
        drop(value);
        assert_eq!(drops.get(), 1);

        // drop deque 中新写入的 value。
        drop(deque);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn remove_middle_then_drop_deque() {
        let drops = std::rc::Rc::new(std::cell::Cell::new(0));

        let mut deque = MyDeque::new();

        for _ in 0..10 {
            deque.push_back(DropTracker::new(&drops));
        }

        // 移除中间节点。
        let (value, _) = deque.at(5).unwrap().remove();

        assert_eq!(drops.get(), 0);
        assert_eq!(deque.len(), 9);

        // 先让被 remove 出来的值析构。
        drop(value);
        assert_eq!(drops.get(), 1);

        // 剩余 9 个 live node 由 MyDeque::drop 析构。
        drop(deque);
        assert_eq!(drops.get(), 10);
    }
}
