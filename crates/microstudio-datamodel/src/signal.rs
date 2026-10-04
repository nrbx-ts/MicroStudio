use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use crate::attributes::AttributeValue;
use crate::instance::InstanceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignalId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u64);

// one enum, not per-event generics: single conversion path for the lua bridge
#[derive(Debug, Clone, PartialEq)]
pub enum EventArg {
    Instance(InstanceId),
    Parent(Option<InstanceId>),
    Attribute(String, AttributeValue),
    Number(f64),
    Text(String),
}

type Listener = Rc<dyn Fn(&[EventArg])>;

#[derive(Default)]
struct SignalState {
    listeners: Vec<(ConnectionId, Listener, bool)>,
}

// listeners snapshotted before invoke: safe to connect/disconnect/fire during dispatch
// fire order is connection order, like Roblox
#[derive(Clone)]
pub struct Signal {
    id: SignalId,
    // a topic name is only known at runtime, so the name is owned
    name: String,
    state: Rc<RefCell<SignalState>>,
    next_connection: Rc<RefCell<u64>>,
}

impl fmt::Debug for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signal")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Signal {
    pub fn new(id: SignalId, name: &str) -> Self {
        Self {
            id,
            name: name.to_string(),
            state: Rc::new(RefCell::new(SignalState::default())),
            next_connection: Rc::new(RefCell::new(1)),
        }
    }

    #[inline]
    pub fn id(&self) -> SignalId {
        self.id
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn connect<F>(&self, listener: F) -> Connection
    where
        F: Fn(&[EventArg]) + 'static,
    {
        self.connect_inner(Rc::new(listener), false)
    }

    pub fn connect_once<F>(&self, listener: F) -> Connection
    where
        F: Fn(&[EventArg]) + 'static,
    {
        self.connect_inner(Rc::new(listener), true)
    }

    fn connect_inner(&self, listener: Listener, once: bool) -> Connection {
        let id = {
            let mut next = self.next_connection.borrow_mut();
            let id = ConnectionId(*next);
            *next += 1;
            id
        };
        self.state.borrow_mut().listeners.push((id, listener, once));
        Connection {
            id,
            signal: self.clone(),
        }
    }

    pub fn disconnect(&self, id: ConnectionId) {
        self.state.borrow_mut().listeners.retain(|(cid, _, _)| *cid != id);
    }

    #[inline]
    pub fn connection_count(&self) -> usize {
        self.state.borrow().listeners.len()
    }

    pub fn fire(&self, args: &[EventArg]) {
        let snapshot: Vec<(ConnectionId, Listener, bool)> =
            self.state.borrow().listeners.clone();

        for (id, listener, once) in snapshot {
            if once {
                self.disconnect(id);
            }
            listener(args);
        }
    }
}

// Drop does nothing: connections live until disconnected or signal dropped
pub struct Connection {
    id: ConnectionId,
    signal: Signal,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("id", &self.id)
            .field("signal", &self.signal.id)
            .finish()
    }
}

impl Connection {
    #[inline]
    pub fn id(&self) -> ConnectionId {
        self.id
    }

    #[inline]
    pub fn signal_id(&self) -> SignalId {
        self.signal.id()
    }

    pub fn is_connected(&self) -> bool {
        self.signal
            .state
            .borrow()
            .listeners
            .iter()
            .any(|(cid, _, _)| *cid == self.id)
    }

    pub fn disconnect(&self) {
        self.signal.disconnect(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn signal() -> Signal {
        Signal::new(SignalId(1), "TestSignal")
    }

    #[test]
    fn fires_in_connection_order() {
        let sig = signal();
        let log = Rc::new(RefCell::new(Vec::new()));
        for tag in ["a", "b", "c"] {
            let log = log.clone();
            sig.connect(move |_| log.borrow_mut().push(tag));
        }
        sig.fire(&[]);
        assert_eq!(*log.borrow(), vec!["a", "b", "c"]);
    }

    #[test]
    fn disconnect_stops_delivery() {
        let sig = signal();
        let hits = Rc::new(Cell::new(0));
        let hits2 = hits.clone();
        let conn = sig.connect(move |_| hits2.set(hits2.get() + 1));
        sig.fire(&[]);
        conn.disconnect();
        sig.fire(&[]);
        assert_eq!(hits.get(), 1);
        assert!(!conn.is_connected());
    }

    #[test]
    fn once_listener_runs_exactly_once() {
        let sig = signal();
        let hits = Rc::new(Cell::new(0));
        let hits2 = hits.clone();
        sig.connect_once(move |_| hits2.set(hits2.get() + 1));
        sig.fire(&[]);
        sig.fire(&[]);
        assert_eq!(hits.get(), 1);
    }

    #[test]
    fn listener_can_disconnect_during_fire() {
        let sig = signal();
        let hits = Rc::new(Cell::new(0));
        let hits2 = hits.clone();
        let conn = Rc::new(sig.connect(move |_| hits2.set(hits2.get() + 1)));
        let conn2 = conn.clone();
        // self-disconnect mid-dispatch must not panic
        let conn3 = conn2.clone();
        sig.connect(move |_| conn3.disconnect());
        sig.fire(&[]);
        assert_eq!(hits.get(), 1);
    }

    #[test]
    fn arguments_are_delivered() {
        let sig = signal();
        let seen = Rc::new(RefCell::new(None));
        let seen2 = seen.clone();
        sig.connect(move |args| *seen2.borrow_mut() = Some(args.to_vec()));
        sig.fire(&[EventArg::Number(42.0)]);
        assert_eq!(*seen.borrow(), Some(vec![EventArg::Number(42.0)]));
    }
}
