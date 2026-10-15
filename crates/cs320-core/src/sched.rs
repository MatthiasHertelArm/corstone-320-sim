//! Event scheduler.
//!
//! Every event source owns a fixed [`Slot`]. Scheduling a slot replaces its previous time, so a
//! slot has at most one pending event. Stale heap entries are skipped by a generation counter.

use crate::time::Ticks;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Identifies an event source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot(pub u16);

#[derive(Debug, Default)]
pub struct Scheduler {
    heap: BinaryHeap<Reverse<(Ticks, u64, u16, u32)>>,
    generation: Vec<u32>,
    armed: Vec<Option<Ticks>>,
    seq: u64,
}

impl Scheduler {
    pub fn new(slots: usize) -> Self {
        Self { heap: BinaryHeap::new(), generation: vec![0; slots], armed: vec![None; slots], seq: 0 }
    }

    /// Schedules `slot` at time `at`, replacing a pending event of the same slot.
    pub fn schedule(&mut self, slot: Slot, at: Ticks) {
        let i = slot.0 as usize;
        self.generation[i] = self.generation[i].wrapping_add(1);
        self.armed[i] = Some(at);
        self.seq += 1;
        self.heap.push(Reverse((at, self.seq, slot.0, self.generation[i])));
    }

    /// Cancels the pending event of `slot`.
    pub fn cancel(&mut self, slot: Slot) {
        let i = slot.0 as usize;
        if self.armed[i].is_some() {
            self.generation[i] = self.generation[i].wrapping_add(1);
            self.armed[i] = None;
        }
    }

    /// Time of the pending event of `slot`.
    pub fn when(&self, slot: Slot) -> Option<Ticks> {
        self.armed[slot.0 as usize]
    }

    fn drop_stale(&mut self) {
        while let Some(Reverse((_, _, s, g))) = self.heap.peek() {
            if self.generation[*s as usize] == *g && self.armed[*s as usize].is_some() {
                break;
            }
            self.heap.pop();
        }
    }

    /// Time of the earliest pending event.
    pub fn next_time(&mut self) -> Option<Ticks> {
        self.drop_stale();
        self.heap.peek().map(|Reverse((t, ..))| *t)
    }

    /// Removes and returns the earliest event due at or before `now`.
    pub fn pop_due(&mut self, now: Ticks) -> Option<(Slot, Ticks)> {
        self.drop_stale();
        match self.heap.peek() {
            Some(Reverse((t, _, s, _))) if *t <= now => {
                let (t, s) = (*t, *s);
                self.heap.pop();
                self.armed[s as usize] = None;
                Some((Slot(s), t))
            }
            _ => None,
        }
    }

    /// Cancels all events.
    pub fn clear(&mut self) {
        for i in 0..self.armed.len() {
            self.cancel(Slot(i as u16));
        }
        self.heap.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_and_replace() {
        let mut s = Scheduler::new(4);
        s.schedule(Slot(1), 100);
        s.schedule(Slot(2), 50);
        s.schedule(Slot(1), 30);
        assert_eq!(s.next_time(), Some(30));
        assert_eq!(s.pop_due(1000), Some((Slot(1), 30)));
        assert_eq!(s.pop_due(1000), Some((Slot(2), 50)));
        assert_eq!(s.pop_due(1000), None);
    }

    #[test]
    fn cancel() {
        let mut s = Scheduler::new(2);
        s.schedule(Slot(0), 10);
        s.cancel(Slot(0));
        assert_eq!(s.next_time(), None);
    }

    #[test]
    fn same_time_fifo() {
        let mut s = Scheduler::new(3);
        s.schedule(Slot(2), 5);
        s.schedule(Slot(0), 5);
        assert_eq!(s.pop_due(5).unwrap().0, Slot(2));
        assert_eq!(s.pop_due(5).unwrap().0, Slot(0));
    }
}
