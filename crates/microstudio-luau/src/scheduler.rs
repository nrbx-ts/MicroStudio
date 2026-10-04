// scheduler stores threads but never drives them; resuming lives in vm
// order is (deadline, insertion_seq), so runs interleave deterministically

use std::collections::VecDeque;

use mlua::{MultiValue, Thread};
use microstudio_datamodel::{InstanceId, SignalId};

use crate::clock::Clock;

// 60 hz; task.wait() with no args waits this long
pub const FRAME_TIME: f64 = 1.0 / 60.0;

pub enum ResumeKind {
    None,
    // task.wait() returns this, matching roblox
    Elapsed { since: f64 },
    // task.spawn/defer/delay args delivered on first resume
    Values(MultiValue),
}

// owner is the script that `script` resolves to
pub struct Entry {
    pub thread: Thread,
    pub owner: Option<InstanceId>,
    pub resume: ResumeKind,
}

impl Entry {
    pub fn ready(thread: Thread, owner: Option<InstanceId>) -> Self {
        Self {
            thread,
            owner,
            resume: ResumeKind::None,
        }
    }
}

struct Timer {
    deadline: f64,
    seq: u64,
    entry: Entry,
}

impl PartialEq for Timer {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.seq == other.seq
    }
}

impl Eq for Timer {}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Timer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // reversed: heap holds min deadline at the top
        other
            .deadline
            .partial_cmp(&self.deadline)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

struct Waiter {
    signal_id: SignalId,
    entry: Entry,
}

pub struct Scheduler {
    clock: Box<dyn Clock>,
    seq: u64,
    ready: VecDeque<Entry>,
    deferred: VecDeque<Entry>,
    timers: Vec<Timer>,
    waiters: Vec<Waiter>,
    // pushed on every resume; `script` must be right when handlers nest
    script_stack: Vec<Option<InstanceId>>,
    warnings: Vec<String>,
}

impl Scheduler {
    pub fn new(clock: Box<dyn Clock>) -> Self {
        Self {
            clock,
            seq: 0,
            ready: VecDeque::new(),
            deferred: VecDeque::new(),
            timers: Vec::new(),
            waiters: Vec::new(),
            script_stack: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn with_virtual_clock() -> Self {
        Self::new(Box::new(crate::clock::VirtualClock::new()))
    }

    #[inline]
    pub fn now(&self) -> f64 {
        self.clock.now()
    }

    #[inline]
    pub fn clock_kind(&self) -> &'static str {
        self.clock.kind()
    }

    #[inline]
    pub fn clock_is_virtual(&self) -> bool {
        self.clock.is_virtual()
    }

    pub fn set_clock(&mut self, clock: Box<dyn Clock>) {
        self.clock = clock;
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    pub fn push_ready(&mut self, entry: Entry) {
        self.ready.push_back(entry);
    }

    pub fn push_deferred(&mut self, entry: Entry) {
        self.deferred.push_back(entry);
    }

    pub fn push_delay(&mut self, delay: f64, mut entry: Entry) {
        let deadline = self.now() + delay.max(0.0);
        let resume = std::mem::replace(&mut entry.resume, ResumeKind::None);
        entry.resume = match resume {
            ResumeKind::Elapsed { .. } => ResumeKind::Elapsed {
                since: self.now(),
            },
            other => other,
        };
        let seq = self.next_seq();
        self.timers.push(Timer {
            deadline,
            seq,
            entry,
        });
    }

    // thread_ptr is Thread::state()
    pub fn cancel(&mut self, thread_ptr: usize) {
        self.ready.retain(|entry| entry.thread.state() as usize != thread_ptr);
        self.deferred
            .retain(|entry| entry.thread.state() as usize != thread_ptr);
        self.timers
            .retain(|timer| timer.entry.thread.state() as usize != thread_ptr);
        self.waiters
            .retain(|waiter| waiter.entry.thread.state() as usize != thread_ptr);
    }

    pub fn pop_ready(&mut self) -> Option<Entry> {
        self.ready.pop_front()
    }

    pub fn has_ready(&self) -> bool {
        !self.ready.is_empty()
    }

    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    pub fn pop_deferred(&mut self) -> Option<Entry> {
        self.deferred.pop_front()
    }

    pub fn has_deferred(&self) -> bool {
        !self.deferred.is_empty()
    }

    pub fn park_on_signal(&mut self, signal_id: SignalId, entry: Entry) {
        self.waiters.push(Waiter { signal_id, entry });
    }

    // removed here: a parked :Wait() is already satisfied, must not resume twice
    pub fn take_waiters(&mut self, signal_id: SignalId) -> Vec<Entry> {
        let mut taken = Vec::new();
        let mut keep = Vec::with_capacity(self.waiters.len());
        for waiter in self.waiters.drain(..) {
            if waiter.signal_id == signal_id {
                taken.push(waiter.entry);
            } else {
                keep.push(waiter);
            }
        }
        self.waiters = keep;
        taken
    }

    pub fn waiter_count(&self) -> usize {
        self.waiters.len()
    }

    pub fn next_deadline(&self) -> Option<f64> {
        self.timers.iter().map(|t| t.deadline).reduce(f64::min)
    }

    pub fn take_due(&mut self, now: f64) -> Vec<Entry> {
        let mut due: Vec<Timer> = Vec::new();
        let mut keep = Vec::with_capacity(self.timers.len());
        for timer in self.timers.drain(..) {
            if timer.deadline <= now {
                due.push(timer);
            } else {
                keep.push(timer);
            }
        }
        self.timers = keep;
        due.sort_by(|a, b| {
            a.deadline
                .partial_cmp(&b.deadline)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.seq.cmp(&b.seq))
        });
        due.into_iter().map(|t| t.entry).collect()
    }

    pub fn timer_count(&self) -> usize {
        self.timers.len()
    }

    pub fn advance_clock_to(&mut self, seconds: f64) {
        self.clock.set_now(seconds);
    }

    pub fn push_script(&mut self, owner: Option<InstanceId>) {
        self.script_stack.push(owner);
    }

    pub fn pop_script(&mut self) -> Option<Option<InstanceId>> {
        self.script_stack.pop()
    }

    pub fn current_script(&self) -> Option<InstanceId> {
        self.script_stack.iter().rev().find_map(|o| *o)
    }

    pub fn script_depth(&self) -> usize {
        self.script_stack.len()
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("clock", &self.clock.kind())
            .field("now", &self.now())
            .field("ready", &self.ready.len())
            .field("deferred", &self.deferred.len())
            .field("timers", &self.timers.len())
            .field("waiters", &self.waiters.len())
            .finish()
    }
}
