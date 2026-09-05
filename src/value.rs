//! Opaque shared ownership and task input/output value containers.
//!
//! Values are represented internally as shared, type-erased allocations. The
//! public API never exposes that representation: tasks exchange [`Shared`]
//! handles, while the executor validates the complete input/output contract.

use crate::{
    error::{ErrorContext, NodeError, NodeErrorKind},
    handles::{InputKey, OutputKey},
    mode::{Mode, SendMode, ValueFor},
    schema::{Cardinality, InputSpec, Presence},
};
use std::{
    any::{Any, TypeId},
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
};

/// Shared ownership of a slot value.
///
/// The representation is intentionally opaque. Its mode marker makes a
/// `Shared<T, Local>` local even though the implementation uses `Arc`.
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

/// Object-safe operations needed after erasing the concrete slot payload.
trait ErasedShared: Any {
    fn value_type_id(&self) -> TypeId;
    fn clone_erased(&self) -> Box<dyn ErasedShared>;
    fn as_any(&self) -> &dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Any, M: Mode> ErasedShared for Shared<T, M> {
    fn value_type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
    fn clone_erased(&self) -> Box<dyn ErasedShared> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

/// One type-erased, shared slot value. The erased object retains the actual
/// `Shared<T, M>` wrapper, so reports can safely borrow it without rebuilding a
/// typed handle or relying on representation casts.
pub(crate) struct StoredValue(Box<dyn ErasedShared>);

// `StoredValue` is always enclosed by a mode-parameterized public container.
// Send-mode construction only accepts `ValueFor<SendMode>`, whose values and
// `Shared` wrappers are Send + Sync. The erased trait cannot express that
// conditional invariant, so the enclosing mode marker remains the gate that
// prevents local values from crossing threads.
unsafe impl Send for StoredValue {}
unsafe impl Sync for StoredValue {}
impl Clone for StoredValue {
    fn clone(&self) -> Self {
        Self(self.0.clone_erased())
    }
}
impl StoredValue {
    pub(crate) fn type_id(&self) -> TypeId {
        self.0.value_type_id()
    }
    pub(crate) fn shared<T: Any, M: Mode>(&self) -> Option<&Shared<T, M>> {
        self.0.as_any().downcast_ref::<Shared<T, M>>()
    }
    pub(crate) fn into_shared<T: Any, M: Mode>(self) -> Option<Shared<T, M>> {
        self.0
            .into_any()
            .downcast::<Shared<T, M>>()
            .ok()
            .map(|value| *value)
    }
    pub(crate) fn from_value<T: ValueFor<M>, M: Mode>(value: T) -> Self {
        Self(Box::new(Shared::<T, M>::new(value)))
    }
    pub(crate) fn from_shared<T: ValueFor<M>, M: Mode>(value: Shared<T, M>) -> Self {
        Self(Box::new(value))
    }
}

/// The address chosen by a task when constructing an output bag.
#[derive(Clone, Debug)]
pub(crate) enum OutputAddress {
    Name(String),
    Key { layout: u64, index: usize },
}
/// One output retained in insertion order. Duplicate addresses are deliberate:
/// complete validation detects them atomically at commit time.
#[derive(Clone)]
pub(crate) struct OutputEntry {
    pub(crate) address: OutputAddress,
    pub(crate) value: StoredValue,
}

/// An uncommitted heterogeneous output bag.
pub struct NodeOutputs<M: Mode> {
    entries: Vec<OutputEntry>,
    _mode: PhantomData<M>,
}
impl<M: Mode> NodeOutputs<M> {
    /// Creates an empty uncommitted output bag.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            _mode: PhantomData,
        }
    }
    /// Shorthand for an empty output bag, suitable for a side-effect-only task.
    pub fn empty() -> Self {
        Self::new()
    }
    /// Returns output attempts in task insertion order for atomic validation.
    pub(crate) fn into_entries(self) -> Vec<OutputEntry> {
        self.entries
    }
}
impl<M: Mode> NodeOutputs<M> {
    /// Adds a named owned value; schema validation occurs at commit time.
    pub fn insert<T: ValueFor<M>>(&mut self, name: impl Into<String>, value: T) {
        self.entries.push(OutputEntry {
            address: OutputAddress::Name(name.into()),
            value: StoredValue::from_value::<T, M>(value),
        });
    }
    /// Adds existing shared ownership without nesting `Shared` inside another value.
    pub fn insert_shared<T: ValueFor<M>>(&mut self, name: impl Into<String>, value: Shared<T, M>) {
        self.entries.push(OutputEntry {
            address: OutputAddress::Name(name.into()),
            value: StoredValue::from_shared::<T, M>(value),
        });
    }
    /// Adds an owned output by a pre-bound key.
    pub fn insert_key<T: ValueFor<M>>(&mut self, key: OutputKey<T>, value: T) {
        self.entries.push(OutputEntry {
            address: OutputAddress::Key {
                layout: key.layout(),
                index: key.index(),
            },
            value: StoredValue::from_value::<T, M>(value),
        });
    }
    /// Adds shared ownership by pre-bound key.
    pub fn insert_shared_key<T: ValueFor<M>>(&mut self, key: OutputKey<T>, value: Shared<T, M>) {
        self.entries.push(OutputEntry {
            address: OutputAddress::Key {
                layout: key.layout(),
                index: key.index(),
            },
            value: StoredValue::from_shared::<T, M>(value),
        });
    }
}
impl<M: Mode> Default for NodeOutputs<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned snapshot of resolved task inputs, safe to move across an await point.
pub struct NodeInputs<M: Mode> {
    layout: u64,
    specs: Vec<InputSpec>,
    values: Vec<Vec<StoredValue>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> NodeInputs<M> {
    /// Builds a resolved input snapshot. The runtime has established mode-valid values.
    pub(crate) fn from_resolved(
        layout: u64,
        specs: Vec<InputSpec>,
        values: Vec<Vec<StoredValue>>,
    ) -> Self {
        Self {
            layout,
            specs,
            values,
            _mode: PhantomData,
        }
    }
    fn invalid() -> NodeError<M> {
        NodeError::internal(NodeErrorKind::InvalidInputs, ErrorContext::default())
    }
    fn named_index(&self, name: &str) -> Result<usize, NodeError<M>> {
        self.specs
            .iter()
            .position(|spec| spec.name == name)
            .ok_or_else(Self::invalid)
    }
    fn key_index<T: ValueFor<M>>(&self, key: InputKey<T>) -> Result<usize, NodeError<M>> {
        if key.layout() != self.layout
            || key.index() >= self.specs.len()
            || self.specs[key.index()].value_type != crate::handles::SlotTypeId::of::<T>()
        {
            return Err(Self::invalid());
        }
        Ok(key.index())
    }
    fn one<T: ValueFor<M>>(
        &self,
        index: usize,
        required: bool,
    ) -> Result<Option<Shared<T, M>>, NodeError<M>> {
        let spec = self.specs.get(index).ok_or_else(Self::invalid)?;
        if spec.value_type != crate::handles::SlotTypeId::of::<T>()
            || spec.cardinality != Cardinality::One
            || (required && spec.presence != Presence::Required)
            || (!required && spec.presence != Presence::Optional)
        {
            return Err(Self::invalid());
        }
        let values = self.values.get(index).ok_or_else(Self::invalid)?;
        if values.len() > 1 {
            return Err(Self::invalid());
        }
        match values.first().cloned() {
            None => Ok(None),
            Some(value) => value.into_shared().map(Some).ok_or_else(Self::invalid),
        }
    }
    fn many_at<T: ValueFor<M>>(&self, index: usize) -> Result<Vec<Shared<T, M>>, NodeError<M>> {
        let spec = self.specs.get(index).ok_or_else(Self::invalid)?;
        if spec.value_type != crate::handles::SlotTypeId::of::<T>()
            || spec.cardinality != Cardinality::Many
        {
            return Err(Self::invalid());
        }
        self.values
            .get(index)
            .ok_or_else(Self::invalid)?
            .iter()
            .cloned()
            .map(|value| value.into_shared().ok_or_else(Self::invalid))
            .collect()
    }
    /// Reads a Required One input, failing on an invalid name, type, or shape.
    pub fn required<T: ValueFor<M>>(&self, name: &str) -> Result<Shared<T, M>, NodeError<M>> {
        self.one(self.named_index(name)?, true)?
            .ok_or_else(Self::invalid)
    }
    /// Reads an Optional One input; absent sources resolve to `None`.
    pub fn optional<T: ValueFor<M>>(
        &self,
        name: &str,
    ) -> Result<Option<Shared<T, M>>, NodeError<M>> {
        self.one(self.named_index(name)?, false)
    }
    /// Reads a Many input in connection order (or external insertion order).
    pub fn many<T: ValueFor<M>>(&self, name: &str) -> Result<Vec<Shared<T, M>>, NodeError<M>> {
        self.many_at(self.named_index(name)?)
    }
    /// Reads a Required One input by pre-bound key without a name lookup.
    pub fn required_key<T: ValueFor<M>>(
        &self,
        key: InputKey<T>,
    ) -> Result<Shared<T, M>, NodeError<M>> {
        self.one(self.key_index(key)?, true)?
            .ok_or_else(Self::invalid)
    }
    /// Reads an Optional One input by pre-bound key.
    pub fn optional_key<T: ValueFor<M>>(
        &self,
        key: InputKey<T>,
    ) -> Result<Option<Shared<T, M>>, NodeError<M>> {
        self.one(self.key_index(key)?, false)
    }
    /// Reads a Many input by pre-bound key in stable binding order.
    pub fn many_key<T: ValueFor<M>>(
        &self,
        key: InputKey<T>,
    ) -> Result<Vec<Shared<T, M>>, NodeError<M>> {
        self.many_at(self.key_index(key)?)
    }
}

// `StoredValue` erases auto-traits. Only this sealed, crate-private marker may
// opt a container back into cross-thread transport; construction paths for the
// sole implementation accept `ValueFor<SendMode>` values only.
trait SendModeStorage: Mode + Send + Sync {}
impl SendModeStorage for SendMode {}

unsafe impl<M: SendModeStorage> Send for NodeInputs<M> {}
unsafe impl<M: SendModeStorage> Sync for NodeInputs<M> {}
unsafe impl<M: SendModeStorage> Send for NodeOutputs<M> {}
unsafe impl<M: SendModeStorage> Sync for NodeOutputs<M> {}
