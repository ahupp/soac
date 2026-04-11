use std::cell::UnsafeCell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CounterEntry<T> {
    pub value: T,
    pub approx_count: u64,
    pub max_overcount: u64,
}

impl<T> CounterEntry<T> {
    pub fn lower_bound(&self) -> u64 {
        self.approx_count.saturating_sub(self.max_overcount)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CounterSlot<T> {
    value: T,
    approx_count: u64,
    max_overcount: u64,
}

/// Fixed-capacity heavy-hitter counter using the space-saving algorithm.
///
/// The counter tracks up to `N` distinct values. Once all `N` slots are full,
/// a new distinct value replaces the current minimum-count slot. The stored
/// `approx_count` is an upper bound on the true count, and `max_overcount`
/// captures the maximum amount by which the value may have been overcounted
/// when it entered the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counter<const N: usize, T> {
    slots: [Option<CounterSlot<T>>; N],
}

impl<const N: usize, T> Default for Counter<N, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, T> Counter<N, T> {
    pub fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
        }
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn entries(&self) -> Vec<CounterEntry<&T>> {
        let mut entries = self
            .slots
            .iter()
            .flatten()
            .map(|slot| CounterEntry {
                value: &slot.value,
                approx_count: slot.approx_count,
                max_overcount: slot.max_overcount,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|lhs, rhs| rhs.approx_count.cmp(&lhs.approx_count));
        entries
    }

    pub fn snapshot(&self) -> Vec<CounterEntry<T>>
    where
        T: Clone,
    {
        self.entries()
            .into_iter()
            .map(|entry| CounterEntry {
                value: entry.value.clone(),
                approx_count: entry.approx_count,
                max_overcount: entry.max_overcount,
            })
            .collect()
    }

    fn min_slot_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.as_ref().map(|slot| (index, slot.approx_count)))
            .min_by_key(|(_, approx_count)| *approx_count)
            .map(|(index, _)| index)
    }
}

impl<const N: usize, T: Eq> Counter<N, T> {
    pub fn record(&mut self, value: T) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.value == value)
        {
            slot.approx_count += 1;
            return;
        }

        if let Some(empty_slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *empty_slot = Some(CounterSlot {
                value,
                approx_count: 1,
                max_overcount: 0,
            });
            return;
        }

        let Some(index) = self.min_slot_index() else {
            return;
        };
        let displaced_count = self.slots[index]
            .as_ref()
            .map(|slot| slot.approx_count)
            .unwrap_or(0);
        self.slots[index] = Some(CounterSlot {
            value,
            approx_count: displaced_count + 1,
            max_overcount: displaced_count,
        });
    }

    pub fn approx_count(&self, value: &T) -> Option<u64> {
        self.slots
            .iter()
            .flatten()
            .find(|slot| &slot.value == value)
            .map(|slot| slot.approx_count)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TopValueCounterSlot {
    occupied: u64,
    value: u64,
    approx_count: u64,
    max_overcount: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopValueCounter {
    slots: [TopValueCounterSlot; 2],
}

impl Default for TopValueCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl TopValueCounter {
    pub const fn new() -> Self {
        Self {
            slots: [TopValueCounterSlot {
                occupied: 0,
                value: 0,
                approx_count: 0,
                max_overcount: 0,
            }; 2],
        }
    }

    pub const fn capacity(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|slot| slot.occupied == 0)
    }

    pub fn snapshot(&self) -> Vec<CounterEntry<u64>> {
        let mut entries = self
            .slots
            .iter()
            .filter(|slot| slot.occupied != 0)
            .map(|slot| CounterEntry {
                value: slot.value,
                approx_count: slot.approx_count,
                max_overcount: slot.max_overcount,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|lhs, rhs| rhs.approx_count.cmp(&lhs.approx_count));
        entries
    }

    fn min_slot_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.occupied != 0)
            .min_by_key(|(_, slot)| slot.approx_count)
            .map(|(index, _)| index)
    }

    pub fn record(&mut self, value: u64) {
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.occupied != 0 && slot.value == value)
        {
            slot.approx_count += 1;
            return;
        }

        if let Some(empty_slot) = self.slots.iter_mut().find(|slot| slot.occupied == 0) {
            *empty_slot = TopValueCounterSlot {
                occupied: 1,
                value,
                approx_count: 1,
                max_overcount: 0,
            };
            return;
        }

        let Some(index) = self.min_slot_index() else {
            return;
        };
        let displaced_count = self.slots[index].approx_count;
        self.slots[index] = TopValueCounterSlot {
            occupied: 1,
            value,
            approx_count: displaced_count + 1,
            max_overcount: displaced_count,
        };
    }
}

#[repr(transparent)]
#[derive(Debug)]
pub struct GilTopValueCounter {
    inner: UnsafeCell<TopValueCounter>,
}

impl Default for GilTopValueCounter {
    fn default() -> Self {
        Self::new()
    }
}

impl GilTopValueCounter {
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(TopValueCounter::new()),
        }
    }

    pub fn as_raw_ptr(&self) -> *mut TopValueCounter {
        self.inner.get()
    }

    pub unsafe fn snapshot_with_gil(&self) -> Vec<CounterEntry<u64>> {
        unsafe { (*self.inner.get()).snapshot() }
    }
}

unsafe impl Send for GilTopValueCounter {}
unsafe impl Sync for GilTopValueCounter {}

#[cfg(test)]
mod tests {
    use super::{Counter, CounterEntry, TopValueCounter};

    #[test]
    fn records_distinct_values_until_capacity() {
        let mut counter = Counter::<3, &'static str>::new();
        counter.record("alpha");
        counter.record("beta");
        counter.record("gamma");

        assert_eq!(
            counter.entries(),
            vec![
                CounterEntry {
                    value: &"alpha",
                    approx_count: 1,
                    max_overcount: 0,
                },
                CounterEntry {
                    value: &"beta",
                    approx_count: 1,
                    max_overcount: 0,
                },
                CounterEntry {
                    value: &"gamma",
                    approx_count: 1,
                    max_overcount: 0,
                },
            ]
        );
    }

    #[test]
    fn increments_existing_values() {
        let mut counter = Counter::<3, &'static str>::new();
        counter.record("alpha");
        counter.record("beta");
        counter.record("alpha");
        counter.record("alpha");

        assert_eq!(counter.approx_count(&"alpha"), Some(3));
        assert_eq!(counter.approx_count(&"beta"), Some(1));
        assert_eq!(
            counter.entries().first(),
            Some(&CounterEntry {
                value: &"alpha",
                approx_count: 3,
                max_overcount: 0,
            })
        );
    }

    #[test]
    fn replaces_lowest_count_once_full() {
        let mut counter = Counter::<2, &'static str>::new();
        counter.record("alpha");
        counter.record("alpha");
        counter.record("beta");
        counter.record("gamma");

        assert_eq!(
            counter.snapshot(),
            vec![
                CounterEntry {
                    value: "alpha",
                    approx_count: 2,
                    max_overcount: 0,
                },
                CounterEntry {
                    value: "gamma",
                    approx_count: 2,
                    max_overcount: 1,
                },
            ]
        );
        assert_eq!(counter.approx_count(&"beta"), None);
    }

    #[test]
    fn keeps_heavy_hitters_in_small_stream() {
        let mut counter = Counter::<2, &'static str>::new();
        for value in ["alpha", "beta", "alpha", "gamma", "alpha", "beta", "beta"] {
            counter.record(value);
        }

        let snapshot = counter.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|entry| {
            entry.value == "alpha" && entry.approx_count == 3 && entry.max_overcount == 0
        }));
        assert!(snapshot.iter().any(|entry| {
            entry.value == "beta" && entry.approx_count == 4 && entry.max_overcount == 2
        }));
        assert_eq!(counter.approx_count(&"gamma"), None);
    }

    #[test]
    fn lower_bound_reflects_overcount_budget() {
        let entry = CounterEntry {
            value: "alpha",
            approx_count: 7,
            max_overcount: 3,
        };

        assert_eq!(entry.lower_bound(), 4);
    }

    #[test]
    fn zero_capacity_counter_ignores_inputs() {
        let mut counter = Counter::<0, &'static str>::new();
        counter.record("alpha");
        counter.record("beta");

        assert!(counter.is_empty());
        assert!(counter.entries().is_empty());
        assert_eq!(counter.approx_count(&"alpha"), None);
    }

    #[test]
    fn top_value_counter_keeps_heavy_hitters() {
        let mut counter = TopValueCounter::new();
        for value in [11, 12, 11, 13, 11, 12, 12] {
            counter.record(value);
        }

        let snapshot = counter.snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|entry| {
            entry.value == 11 && entry.approx_count == 3 && entry.max_overcount == 0
        }));
        assert!(snapshot.iter().any(|entry| {
            entry.value == 12 && entry.approx_count == 4 && entry.max_overcount == 2
        }));
    }

    #[test]
    fn top_value_counter_zero_init_is_empty() {
        let counter = TopValueCounter::new();
        assert_eq!(counter.capacity(), 2);
        assert!(counter.is_empty());
        assert!(counter.snapshot().is_empty());
    }
}
