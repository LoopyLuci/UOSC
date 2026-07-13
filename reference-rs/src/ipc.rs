//! Inter-process communication — real port of `kernel/ipc.ti`.
//!
//! Two real gaps closed relative to the Titan source:
//!
//! 1. `get_process_capability` (`kernel/ipc.ti:257-277`) ignores its `pid`
//!    and `resource` arguments and unconditionally returns a token with
//!    every permission bit granted — meaning nothing in the specified
//!    `send_message`/`receive_message` can ever actually deny access. This
//!    port threads a real [`CapabilityBroker`](crate::capability::CapabilityBroker)
//!    through both and denies exactly like every other resource type.
//! 2. `RingBuffer` in the Titan source stores serialized byte blobs behind
//!    a capacity check (`(write_pos + data.len()) % capacity == read_pos`)
//!    that doesn't actually detect "full" correctly for a wrapping buffer,
//!    and its `dequeue` reads a message-length prefix from only 2 of the
//!    "8 bytes total" the comment says it needs. [`RingBuffer`] below is a
//!    real, capacity-correct single-producer/single-consumer queue of
//!    [`Message`] values — verified under real concurrent producer/consumer
//!    threads in this module's tests, not just single-threaded logic.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::capability::{CapabilityBroker, EnforcementDecision, Permissions, ResourceType};

pub type Pid = u64;
pub type PortId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub sender: Pid,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    PortNotFound,
    QueueFull,
    QueueEmpty,
    CapabilityDenied,
}

/// A real, capacity-correct single-producer/single-consumer ring buffer.
///
/// Unlike the Titan source, "full" and "empty" are distinguished by an
/// explicit `len` counter rather than an index-equality check that can't
/// tell the two states apart once the write index has wrapped past the
/// read index exactly once.
///
/// # Safety / concurrency model
/// This type is genuinely lock-free for the *single* producer / *single*
/// consumer case: the producer only ever advances `write_index`, the
/// consumer only ever advances `read_index`, and each side only reads the
/// other's index (with `Acquire`) before touching a slot it owns. It is
/// **not** safe for multiple concurrent producers or multiple concurrent
/// consumers without additional synchronization — a multi-sender port
/// would need to serialize `enqueue` calls (e.g. behind the capability
/// broker's own lock) before reaching this structure. That's a deliberate,
/// documented scope boundary, not an oversight.
pub struct RingBuffer {
    slots: Vec<UnsafeCell<Option<Message>>>,
    capacity: usize,
    write_index: AtomicUsize,
    read_index: AtomicUsize,
    len: AtomicUsize,
}

// SAFETY: see the concurrency model doc above — sound for SPSC use.
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        RingBuffer {
            slots: (0..capacity).map(|_| UnsafeCell::new(None)).collect(),
            capacity,
            write_index: AtomicUsize::new(0),
            read_index: AtomicUsize::new(0),
            len: AtomicUsize::new(0),
        }
    }

    /// Producer-side only.
    pub fn enqueue(&self, message: Message) -> Result<(), IpcError> {
        if self.len.load(Ordering::Acquire) >= self.capacity {
            return Err(IpcError::QueueFull);
        }
        let idx = self.write_index.load(Ordering::Relaxed);
        // SAFETY: only the single producer ever writes to `write_index`'s
        // slot, and it has just verified there's room, so this slot cannot
        // be concurrently read by a consumer that hasn't been told (via
        // `len`) that it exists yet.
        unsafe {
            *self.slots[idx % self.capacity].get() = Some(message);
        }
        self.write_index.store(idx + 1, Ordering::Relaxed);
        self.len.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Consumer-side only.
    pub fn dequeue(&self) -> Result<Message, IpcError> {
        if self.len.load(Ordering::Acquire) == 0 {
            return Err(IpcError::QueueEmpty);
        }
        let idx = self.read_index.load(Ordering::Relaxed);
        // SAFETY: symmetric to `enqueue` — `len` being nonzero means the
        // producer has finished writing this slot and released it to us.
        let message = unsafe { (*self.slots[idx % self.capacity].get()).take() };
        self.read_index.store(idx + 1, Ordering::Relaxed);
        self.len.fetch_sub(1, Ordering::Release);
        message.ok_or(IpcError::QueueEmpty)
    }

    pub fn len(&self) -> usize {
        self.len.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Port {
    pub port_id: PortId,
    pub owner_pid: Pid,
    pub queue: RingBuffer,
}

#[derive(Default)]
pub struct PortTable {
    ports: BTreeMap<PortId, Port>,
    next_port_id: PortId,
}

impl PortTable {
    pub fn new() -> Self {
        PortTable { ports: BTreeMap::new(), next_port_id: 1 }
    }

    /// Creates the port and issues the owner a real IPC capability over it
    /// — callers with no capability at all still can't send or receive,
    /// closing the gap described in this module's top-level docs.
    pub fn create_port(
        &mut self,
        owner_pid: Pid,
        capacity: usize,
        caps: &mut CapabilityBroker,
        now: u64,
    ) -> PortId {
        let port_id = self.next_port_id;
        self.next_port_id += 1;
        self.ports.insert(
            port_id,
            Port { port_id, owner_pid, queue: RingBuffer::new(capacity) },
        );
        caps.issue(
            0,
            owner_pid,
            ResourceType::Ipc,
            port_resource_path(port_id),
            Permissions { read: true, write: true, delegate: true, ..Permissions::NONE },
            now,
            u64::MAX,
        );
        port_id
    }

    pub fn delete_port(&mut self, port_id: PortId) -> Result<(), IpcError> {
        self.ports.remove(&port_id).map(|_| ()).ok_or(IpcError::PortNotFound)
    }

    pub fn send_message(
        &mut self,
        sender: Pid,
        dest_port: PortId,
        payload: Vec<u8>,
        caps: &mut CapabilityBroker,
        now: u64,
    ) -> Result<(), IpcError> {
        let decision = caps.check_access(
            sender,
            ResourceType::Ipc,
            &port_resource_path(dest_port),
            Permissions { write: true, ..Permissions::NONE },
            now,
        );
        if decision != EnforcementDecision::Allowed {
            return Err(IpcError::CapabilityDenied);
        }
        let port = self.ports.get(&dest_port).ok_or(IpcError::PortNotFound)?;
        port.queue.enqueue(Message { sender, payload })
    }

    pub fn receive_message(
        &mut self,
        receiver: Pid,
        port_id: PortId,
        caps: &mut CapabilityBroker,
        now: u64,
    ) -> Result<Message, IpcError> {
        let decision = caps.check_access(
            receiver,
            ResourceType::Ipc,
            &port_resource_path(port_id),
            Permissions { read: true, ..Permissions::NONE },
            now,
        );
        if decision != EnforcementDecision::Allowed {
            return Err(IpcError::CapabilityDenied);
        }
        let port = self.ports.get(&port_id).ok_or(IpcError::PortNotFound)?;
        port.queue.dequeue()
    }
}

fn port_resource_path(port_id: PortId) -> alloc::string::String {
    alloc::format!("port:{port_id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityBroker;
    use proptest::prelude::*;

    #[test]
    fn send_without_any_capability_is_denied() {
        let mut caps = CapabilityBroker::new();
        let mut table = PortTable::new();
        // Port created by pid 2, but pid 1 (the sender here) was never granted anything.
        let owner_caps_holder_port = table.create_port(2, 4, &mut caps, 0);
        let result = table.send_message(1, owner_caps_holder_port, alloc::vec![1, 2, 3], &mut caps, 5);
        assert_eq!(result, Err(IpcError::CapabilityDenied));
    }

    #[test]
    fn owner_can_send_and_receive_its_own_port() {
        let mut caps = CapabilityBroker::new();
        let mut table = PortTable::new();
        let port = table.create_port(1, 4, &mut caps, 0);

        table.send_message(1, port, alloc::vec![9, 9], &mut caps, 5).unwrap();
        let msg = table.receive_message(1, port, &mut caps, 6).unwrap();
        assert_eq!(msg.payload, alloc::vec![9, 9]);
        assert_eq!(msg.sender, 1);
    }

    #[test]
    fn receive_on_empty_queue_is_reported_not_blocked_forever() {
        let mut caps = CapabilityBroker::new();
        let mut table = PortTable::new();
        let port = table.create_port(1, 4, &mut caps, 0);
        assert_eq!(table.receive_message(1, port, &mut caps, 5), Err(IpcError::QueueEmpty));
    }

    #[test]
    fn ring_buffer_reports_full_correctly_after_exact_capacity_fill() {
        let rb = RingBuffer::new(2);
        rb.enqueue(Message { sender: 1, payload: alloc::vec![1] }).unwrap();
        rb.enqueue(Message { sender: 1, payload: alloc::vec![2] }).unwrap();
        assert_eq!(
            rb.enqueue(Message { sender: 1, payload: alloc::vec![3] }),
            Err(IpcError::QueueFull)
        );
    }

    #[test]
    fn ring_buffer_preserves_fifo_order_across_many_wraparounds() {
        let rb = RingBuffer::new(3);
        for round in 0..10u8 {
            rb.enqueue(Message { sender: round as u64, payload: alloc::vec![round] }).unwrap();
            rb.enqueue(Message { sender: round as u64, payload: alloc::vec![round, round] }).unwrap();
            let a = rb.dequeue().unwrap();
            let b = rb.dequeue().unwrap();
            assert_eq!(a.payload, alloc::vec![round]);
            assert_eq!(b.payload, alloc::vec![round, round]);
        }
    }

    proptest! {
        /// Any interleaving of enqueue/dequeue (respecting full/empty)
        /// must preserve strict FIFO order — the property the Titan
        /// source's incomplete length-prefix parsing couldn't guarantee.
        #[test]
        fn fifo_order_holds_under_arbitrary_interleavings(ops in prop::collection::vec(0u8..2, 1..60)) {
            let rb = RingBuffer::new(5);
            let mut expected: alloc::collections::VecDeque<u8> = alloc::collections::VecDeque::new();
            let mut next_val: u8 = 0;

            for op in ops {
                if op == 0 {
                    if rb.enqueue(Message { sender: 1, payload: alloc::vec![next_val] }).is_ok() {
                        expected.push_back(next_val);
                        next_val = next_val.wrapping_add(1);
                    }
                } else if let Ok(msg) = rb.dequeue() {
                    prop_assert_eq!(msg.payload, alloc::vec![expected.pop_front().unwrap()]);
                }
            }
        }
    }

    #[test]
    fn real_concurrent_spsc_producer_consumer_loses_and_corrupts_nothing() {
        use std::sync::Arc;
        use std::thread;

        let rb = Arc::new(RingBuffer::new(8));
        let total = 5000u32;

        let producer_rb = Arc::clone(&rb);
        let producer = thread::spawn(move || {
            let mut sent = 0u32;
            while sent < total {
                let bytes = sent.to_le_bytes().to_vec();
                if producer_rb.enqueue(Message { sender: 1, payload: bytes }).is_ok() {
                    sent += 1;
                } else {
                    thread::yield_now();
                }
            }
        });

        let consumer_rb = Arc::clone(&rb);
        let consumer = thread::spawn(move || {
            let mut received = Vec::with_capacity(total as usize);
            while received.len() < total as usize {
                if let Ok(msg) = consumer_rb.dequeue() {
                    let bytes: [u8; 4] = msg.payload.try_into().unwrap();
                    received.push(u32::from_le_bytes(bytes));
                } else {
                    thread::yield_now();
                }
            }
            received
        });

        producer.join().unwrap();
        let received = consumer.join().unwrap();
        let expected: Vec<u32> = (0..total).collect();
        assert_eq!(received, expected, "every message must arrive exactly once, in order");
    }
}
