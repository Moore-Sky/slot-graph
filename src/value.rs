//! Opaque shared ownership and task input/output value containers.
//! Shared is usable now; heterogeneous input/output operations remain unimplemented.
//! Named access is convenient but may resolve strings during execution. Keyed
//! access uses pre-bound layout indices; this does not promise zero allocations.
//!
//! SendMode rejects thread-local values at the value insertion boundary:
//! ```compile_fail
//! use slot_graph::{SendMode, Shared};
//! use std::rc::Rc;
//! let value = Shared::<Rc<u32>, SendMode>::new(Rc::new(7));
//! ```
//!
//! Keyed output types cannot be silently changed:
//! ```compile_fail
//! use slot_graph::{Local, NodeOutputs, OutputKey};
//! fn wrong_type(outputs: &mut NodeOutputs<Local>, key: OutputKey<u32>) {
//!     outputs.insert_key(key, String::from("not a number"));
//! }
//! ```
//!
//! Keys carry metadata, not permission to insert local values in SendMode:
//! ```compile_fail
//! use slot_graph::{NodeOutputs, OutputKey, SendMode};
//! use std::rc::Rc;
//! fn wrong_mode(outputs: &mut NodeOutputs<SendMode>, key: OutputKey<Rc<u32>>) {
//!     outputs.insert_key(key, Rc::new(7));
//! }
//! ```

use crate::{
    error::NodeError,
    handles::{InputKey, OutputKey},
    mode::{Mode, ValueFor},
};
use std::{marker::PhantomData, ops::Deref, sync::Arc};

/// Shared ownership of a slot value.
///
/// The representation is intentionally opaque.  Its mode marker makes a
/// `Shared<T, Local>` local even though the implementation is free to use a
/// different reference-counting strategy internally.
pub struct Shared<T: ?Sized + 'static, M: Mode> {
    value: Arc<T>,
    _mode: PhantomData<M>,
}

impl<T: ?Sized + 'static, M: Mode> Clone for Shared<T, M> {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
            _mode: PhantomData,
        }
    }
}

impl<T: ?Sized + 'static, M: Mode> Deref for Shared<T, M> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: ?Sized + 'static, M: Mode> AsRef<T> for Shared<T, M> {
    /// Borrows the shared value without changing its ownership count.
    fn as_ref(&self) -> &T {
        &self.value
    }
}

impl<T: ValueFor<M>, M: Mode> Shared<T, M> {
    /// Wraps a value satisfying the selected mode's thread-safety requirements.
    pub fn new(value: T) -> Self {
        Self {
            value: Arc::new(value),
            _mode: PhantomData,
        }
    }
}
/// An uncommitted heterogeneous output bag.  Schema validation happens at
/// execution commit time, not in [`crate::outputs!`].
pub struct NodeOutputs<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> NodeOutputs<M> {
    /// Creates an empty uncommitted output bag. Currently unimplemented.
    pub fn new() -> Self {
        unimplemented!()
    }
    /// Shorthand for an empty output bag, suitable for a side-effect-only task.
    pub fn empty() -> Self {
        Self::new()
    }
    /// Adds a named owned value; complete Schema validation occurs at commit.
    /// Currently unimplemented, including duplicate-name tracking.
    pub fn insert<T: ValueFor<M>>(&mut self, name: impl Into<String>, value: T) {
        let _ = (name, value);
        unimplemented!()
    }
    /// Adds existing shared ownership without nesting Shared inside another value.
    /// Currently unimplemented.
    pub fn insert_shared<T: ValueFor<M>>(&mut self, name: impl Into<String>, value: Shared<T, M>) {
        let _ = (name, value);
        unimplemented!()
    }

    /// Adds an owned output by a pre-bound key without a success-path name lookup.
    ///
    /// Validation is deferred to atomic commit: wrong-layout keys, duplicate
    /// writes (including a named/keyed pair addressing the same output), missing
    /// outputs, and type errors fail the node with InvalidOutputs and publish
    /// nothing. Mixed named/keyed bags are allowed but are not lookup-free.
    /// Currently unimplemented.
    pub fn insert_key<T: ValueFor<M>>(&mut self, _key: OutputKey<T>, _value: T) {
        unimplemented!()
    }

    /// Adds shared ownership by key without wrapping Shared inside another value.
    /// Uses the same deferred complete-output validation as [`Self::insert_key`].
    /// Currently unimplemented.
    pub fn insert_shared_key<T: ValueFor<M>>(&mut self, _key: OutputKey<T>, _value: Shared<T, M>) {
        unimplemented!()
    }
}
impl<M: Mode> Default for NodeOutputs<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned snapshot of resolved task inputs, safe to move across an await point.
/// Accessors check names, types, and declared cardinality. They are API stubs.
pub struct NodeInputs<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> NodeInputs<M> {
    /// Reads a Required One input, failing on an invalid name/type/declaration.
    pub fn required<T: ValueFor<M>>(&self, _name: &str) -> Result<Shared<T, M>, NodeError<M>> {
        unimplemented!()
    }
    /// Reads an Optional One input; absent sources resolve to None.
    pub fn optional<T: ValueFor<M>>(
        &self,
        _name: &str,
    ) -> Result<Option<Shared<T, M>>, NodeError<M>> {
        unimplemented!()
    }
    /// Reads a Many input in connection order (or external insertion order).
    pub fn many<T: ValueFor<M>>(&self, _name: &str) -> Result<Vec<Shared<T, M>>, NodeError<M>> {
        unimplemented!()
    }

    /// Reads a Required One input by pre-bound key without name/hash lookup.
    ///
    /// Layout, index, type, and input shape are checked; a mismatch is
    /// InvalidInputs. A key from a different binding is rejected even if its
    /// index and type happen to match. Currently unimplemented.
    pub fn required_key<T: ValueFor<M>>(
        &self,
        _key: InputKey<T>,
    ) -> Result<Shared<T, M>, NodeError<M>> {
        unimplemented!()
    }

    /// Reads an Optional One input by key; absent sources resolve to None.
    /// Uses the layout and shape checks of [`Self::required_key`]. Currently a stub.
    pub fn optional_key<T: ValueFor<M>>(
        &self,
        _key: InputKey<T>,
    ) -> Result<Option<Shared<T, M>>, NodeError<M>> {
        unimplemented!()
    }

    /// Reads either Required or Optional Many by key in stable binding order.
    ///
    /// This avoids name lookup, not allocation or Shared-clone costs. A Many key
    /// cannot be read through a One accessor. Currently unimplemented.
    pub fn many_key<T: ValueFor<M>>(
        &self,
        _key: InputKey<T>,
    ) -> Result<Vec<Shared<T, M>>, NodeError<M>> {
        unimplemented!()
    }
}
