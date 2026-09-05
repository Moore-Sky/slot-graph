//! Graph-scoped identities, typed Slot handles, and delayed name selectors.
//! Typed handles preserve schema generations; names are resolved by graph APIs.
//! InputKey and OutputKey instead address one immutable task layout, not a graph.

use std::{
    any::{Any, TypeId},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
};

/// Process-local identity assigned to one editable graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphId(pub(crate) u64);
/// Identity of an immutable compiled graph version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VersionId(pub u64);
/// Identity of one execution of a compiled graph version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RunId(pub u64);
/// Generational identity of a node in its owning graph.
///
/// Using it with another graph is a `ForeignHandle` error; using it after
/// removal is a stale-handle error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId {
    graph: GraphId,
    raw: u64,
}
/// Generational identity of one connection in its owning graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EdgeId {
    graph: GraphId,
    raw: u64,
}
/// Stable schema-level identity for a slot within one node direction.
///
/// It is not a graph-wide handle and is not used to auto-match slots across
/// different nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotId(pub u64);
/// Process-local runtime type identity carried by schema descriptors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotTypeId(TypeId);

/// Pre-resolved task input address issued by a [`BoundSchema`][crate::BoundSchema].
///
/// A key contains an opaque layout identity and dense, node-local index. It is
/// valid for nodes deliberately registered with that same bound schema (or a
/// clone), including old compiled versions. A separately bound schema rejects
/// it even if the declarations look identical. Accessors also check input shape.
/// Keys carry metadata only: `Copy` and mobility impose no bounds on `T`.
/// Use [`InputSlot`] for graph connections and external inputs instead.
pub struct InputKey<T: ?Sized> {
    layout: u64,
    index: usize,
    _type: PhantomData<fn() -> T>,
}

/// Pre-resolved task output address issued by a [`BoundSchema`][crate::BoundSchema].
///
/// Like [`InputKey`], this is layout-scoped, not graph- or node-scoped. It is
/// not interchangeable with an [`OutputSlot`] used for edges and reports.
/// Keyed output insertion defers validation to the whole output commit.
pub struct OutputKey<T: ?Sized> {
    layout: u64,
    index: usize,
    _type: PhantomData<fn() -> T>,
}

macro_rules! impl_key_traits {
    ($key:ident) => {
        impl<T: ?Sized> Copy for $key<T> {}
        impl<T: ?Sized> Clone for $key<T> {
            fn clone(&self) -> Self {
                *self
            }
        }
        impl<T: ?Sized> PartialEq for $key<T> {
            fn eq(&self, other: &Self) -> bool {
                self.layout == other.layout && self.index == other.index
            }
        }
        impl<T: ?Sized> Eq for $key<T> {}
        impl<T: ?Sized> Hash for $key<T> {
            fn hash<H: Hasher>(&self, state: &mut H) {
                self.layout.hash(state);
                self.index.hash(state);
            }
        }
        impl<T: ?Sized> fmt::Debug for $key<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($key))
                    .field("layout", &self.layout)
                    .field("index", &self.index)
                    .finish()
            }
        }
    };
}
impl_key_traits!(InputKey);
impl_key_traits!(OutputKey);

impl SlotId {
    /// Creates a schema-level slot identity from an application-defined value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}
impl SlotTypeId {
    /// Returns the process-local type identity for `T`.
    ///
    /// This identity is suitable for in-process validation only, not
    /// serialization or cross-binary type matching.
    pub fn of<T: Any>() -> Self {
        Self(TypeId::of::<T>())
    }

    pub(crate) fn matches(self, value: TypeId) -> bool {
        self.0 == value
    }
}

impl GraphId {
    /// Creates a graph identity for internal graph storage.
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }
}

impl NodeId {
    /// Creates a node identity for internal graph storage.
    pub(crate) const fn new(graph: GraphId, raw: u64) -> Self {
        Self { graph, raw }
    }

    /// Returns the owning graph identity.
    pub(crate) const fn graph(self) -> GraphId {
        self.graph
    }

    /// Returns the internal generational value.
    pub(crate) const fn raw(self) -> u64 {
        self.raw
    }
}

impl EdgeId {
    /// Creates an edge identity for internal graph storage.
    pub(crate) const fn new(graph: GraphId, raw: u64) -> Self {
        Self { graph, raw }
    }

    /// Returns the owning graph identity.
    pub(crate) const fn graph(self) -> GraphId {
        self.graph
    }

    /// Returns the internal generational value.
    pub(crate) const fn raw(self) -> u64 {
        self.raw
    }
}

impl<T: ?Sized> InputKey<T> {
    /// Creates a layout-local input key after schema validation.
    pub(crate) fn new(layout: u64, index: usize) -> Self {
        Self {
            layout,
            index,
            _type: PhantomData,
        }
    }

    /// Returns the opaque layout identity.
    pub(crate) const fn layout(self) -> u64 {
        self.layout
    }

    /// Returns the dense input index.
    pub(crate) const fn index(self) -> usize {
        self.index
    }
}

impl<T: ?Sized> OutputKey<T> {
    /// Creates a layout-local output key after schema validation.
    pub(crate) fn new(layout: u64, index: usize) -> Self {
        Self {
            layout,
            index,
            _type: PhantomData,
        }
    }

    /// Returns the opaque layout identity.
    pub(crate) const fn layout(self) -> u64 {
        self.layout
    }

    /// Returns the dense output index.
    pub(crate) const fn index(self) -> usize {
        self.index
    }
}

/// A delayed input-slot lookup by node and name.
///
/// Graph operations resolve it when consumed. Use a typed [`InputSlot`] for a
/// long-lived generation-checked handle.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputSlotSelector {
    node: NodeId,
    name: String,
    slot: Option<SlotId>,
    generation: Option<u64>,
}
/// A delayed output-slot lookup by node and name.
///
/// Graph operations resolve it when consumed. Use a typed [`OutputSlot`] for
/// a long-lived generation-checked handle.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OutputSlotSelector {
    node: NodeId,
    name: String,
    slot: Option<SlotId>,
    generation: Option<u64>,
}

/// A schema-validated, typed input-slot handle.
///
/// It records a schema generation and becomes stale after its node's schema
/// is replaced, even if a compatible edge remains. It is `Copy` and hashable
/// without imposing a trait bound on `T`.
pub struct InputSlot<T: ?Sized> {
    node: NodeId,
    slot: SlotId,
    generation: u64,
    _type: PhantomData<fn() -> T>,
}
/// A schema-validated, typed output-slot handle.
///
/// It records a schema generation and becomes stale after its node's schema
/// is replaced, even if a compatible edge remains. It is `Copy` and hashable
/// without imposing a trait bound on `T`.
pub struct OutputSlot<T: ?Sized> {
    node: NodeId,
    slot: SlotId,
    generation: u64,
    _type: PhantomData<fn() -> T>,
}
impl<T: ?Sized> Copy for InputSlot<T> {}
impl<T: ?Sized> Clone for InputSlot<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for OutputSlot<T> {}
impl<T: ?Sized> Clone for OutputSlot<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> fmt::Debug for InputSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputSlot")
            .field("node", &self.node)
            .field("slot", &self.slot)
            .field("generation", &self.generation)
            .finish()
    }
}
impl<T: ?Sized> fmt::Debug for OutputSlot<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputSlot")
            .field("node", &self.node)
            .field("slot", &self.slot)
            .field("generation", &self.generation)
            .finish()
    }
}
impl<T: ?Sized> PartialEq for InputSlot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.slot == other.slot && self.generation == other.generation
    }
}
impl<T: ?Sized> Eq for InputSlot<T> {}
impl<T: ?Sized> PartialEq for OutputSlot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.slot == other.slot && self.generation == other.generation
    }
}
impl<T: ?Sized> Eq for OutputSlot<T> {}
impl<T: ?Sized> Hash for InputSlot<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.hash(state);
        self.slot.hash(state);
        self.generation.hash(state);
    }
}
impl<T: ?Sized> Hash for OutputSlot<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.hash(state);
        self.slot.hash(state);
        self.generation.hash(state);
    }
}

impl NodeId {
    /// Creates a delayed input selector without accessing a graph.
    ///
    /// The receiving graph operation resolves the name and validates this
    /// node when it consumes the selector.
    pub fn input(self, name: impl Into<String>) -> InputSlotSelector {
        InputSlotSelector {
            node: self,
            name: name.into(),
            slot: None,
            generation: None,
        }
    }
    /// Creates a delayed output selector without accessing a graph.
    ///
    /// The receiving graph operation resolves the name and validates this
    /// node when it consumes the selector.
    pub fn output(self, name: impl Into<String>) -> OutputSlotSelector {
        OutputSlotSelector {
            node: self,
            name: name.into(),
            slot: None,
            generation: None,
        }
    }
}

impl InputSlotSelector {
    /// Returns selector data to graph editing internals.
    pub(crate) fn into_parts(self) -> (NodeId, String, Option<SlotId>, Option<u64>) {
        (self.node, self.name, self.slot, self.generation)
    }
}

impl OutputSlotSelector {
    /// Returns selector data to graph editing internals.
    pub(crate) fn into_parts(self) -> (NodeId, String, Option<SlotId>, Option<u64>) {
        (self.node, self.name, self.slot, self.generation)
    }
}

impl<T: ?Sized> InputSlot<T> {
    /// Creates a validated graph-facing input handle.
    pub(crate) fn new(node: NodeId, slot: SlotId, generation: u64) -> Self {
        Self {
            node,
            slot,
            generation,
            _type: PhantomData,
        }
    }

    /// Returns the handle data to graph and runtime internals.
    pub(crate) const fn parts(self) -> (NodeId, SlotId, u64) {
        (self.node, self.slot, self.generation)
    }
}

impl<T: ?Sized> OutputSlot<T> {
    /// Creates a validated graph-facing output handle.
    pub(crate) fn new(node: NodeId, slot: SlotId, generation: u64) -> Self {
        Self {
            node,
            slot,
            generation,
            _type: PhantomData,
        }
    }

    /// Returns the handle data to graph and report internals.
    pub(crate) const fn parts(self) -> (NodeId, SlotId, u64) {
        (self.node, self.slot, self.generation)
    }
}

impl NodeSelector {
    /// Returns the selection representation to graph editing internals.
    pub(crate) fn into_parts(self) -> Result<NodeId, String> {
        match self {
            Self::Id(node) => Ok(node),
            Self::Name(name) => Err(name),
        }
    }
}

/// Converts a typed input handle into its generation-checked selector form.
impl<T: ?Sized> From<InputSlot<T>> for InputSlotSelector {
    fn from(value: InputSlot<T>) -> Self {
        Self {
            node: value.node,
            name: String::new(),
            slot: Some(value.slot),
            generation: Some(value.generation),
        }
    }
}
/// Converts a typed output handle into its generation-checked selector form.
impl<T: ?Sized> From<OutputSlot<T>> for OutputSlotSelector {
    fn from(value: OutputSlot<T>) -> Self {
        Self {
            node: value.node,
            name: String::new(),
            slot: Some(value.slot),
            generation: Some(value.generation),
        }
    }
}
/// Selects a node by stable identity or its current unique name.
///
/// Names are graph-edit conveniences; compiled versions retain node identity
/// rather than performing name lookup.
pub enum NodeSelector {
    /// Selects the exact graph-scoped node identity.
    Id(NodeId),
    /// Resolves the graph's current unique node name at the operation site.
    Name(String),
}
/// Uses a node identity as a selector.
impl From<NodeId> for NodeSelector {
    fn from(value: NodeId) -> Self {
        Self::Id(value)
    }
}
/// Uses a borrowed node name as a selector.
impl From<&str> for NodeSelector {
    fn from(value: &str) -> Self {
        Self::Name(value.to_owned())
    }
}
/// Uses an owned node name as a selector.
impl From<String> for NodeSelector {
    fn from(value: String) -> Self {
        Self::Name(value)
    }
}
