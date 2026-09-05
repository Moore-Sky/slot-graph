//! Ordered input/output declarations used to validate graph connections.
//! Descriptors contain type metadata, not values. Mode bounds apply when values enter the graph.

use crate::handles::{SlotId, SlotTypeId};
use std::any::Any;

/// States whether an input needs at least one source to compile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presence {
    /// The selected compiled subgraph must provide this input.
    Required,
    /// This input may have no source, but any connected source is still a
    /// normal dependency.
    Optional,
}
/// States how many sources an input accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cardinality {
    /// Accepts at most one source.
    One,
    /// Accepts an ordered collection of sources.
    Many,
}

/// Runtime schema declaration for one input slot.
///
/// The graph validates that input names and slot identities are unique within
/// their direction. `auto_collect` affects only [`Graph::connect_nodes`][crate::Graph::connect_nodes]
/// and never changes an existing edge.
#[derive(Clone, Debug)]
pub struct InputSpec {
    /// Stable semantic identity used to preserve compatible slots across a
    /// schema replacement.
    pub id: SlotId,
    /// Human-readable, node-local lookup name.
    pub name: String,
    /// Process-local runtime type accepted by this input.
    pub value_type: SlotTypeId,
    /// Whether a selected subgraph must provide at least one source.
    pub presence: Presence,
    /// Whether this input accepts one source or an ordered collection.
    pub cardinality: Cardinality,
    /// Permits `connect_nodes` to collect all compatible source outputs for a
    /// `Many` input.
    pub auto_collect: bool,
}
/// Runtime schema declaration for one single-value output slot.
#[derive(Clone, Debug)]
pub struct OutputSpec {
    /// Stable semantic identity used to preserve compatible slots across a
    /// schema replacement.
    pub id: SlotId,
    /// Human-readable, node-local lookup name.
    pub name: String,
    /// Process-local runtime type produced by this output.
    pub value_type: SlotTypeId,
}
/// Ordered input and output declarations for a node.
///
/// The order is observable for schema enumeration and deterministic automatic
/// connection planning. A schema describes types and shape, never values.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    /// Ordered input declarations.
    pub inputs: Vec<InputSpec>,
    /// Ordered output declarations.
    pub outputs: Vec<OutputSpec>,
}

impl InputSpec {
    /// Creates a required input accepting exactly one value of `T`.
    pub fn required_one<T: Any>(name: impl Into<String>) -> Self {
        Self::new::<T>(name, Presence::Required, Cardinality::One)
    }
    /// Creates an optional input accepting zero or one value of `T`.
    pub fn optional_one<T: Any>(name: impl Into<String>) -> Self {
        Self::new::<T>(name, Presence::Optional, Cardinality::One)
    }
    /// Creates a required input accepting one or more values of `T`.
    pub fn required_many<T: Any>(name: impl Into<String>) -> Self {
        Self::new::<T>(name, Presence::Required, Cardinality::Many)
    }
    /// Creates an optional input accepting any number of values of `T`.
    pub fn optional_many<T: Any>(name: impl Into<String>) -> Self {
        Self::new::<T>(name, Presence::Optional, Cardinality::Many)
    }
    /// Creates an input specification with an ID derived from its name.
    ///
    /// Use a struct literal with an explicit [`SlotId`] when a stable identity
    /// must survive a slot rename or declaration reorder.
    pub fn new<T: Any>(
        name: impl Into<String>,
        presence: Presence,
        cardinality: Cardinality,
    ) -> Self {
        let name = name.into();
        Self {
            id: slot_id(&name),
            name,
            value_type: SlotTypeId::of::<T>(),
            presence,
            cardinality,
            auto_collect: false,
        }
    }
    /// Enables or disables automatic collection for this input.
    ///
    /// Graph validation rejects `true` for a [`Cardinality::One`] input.
    pub fn auto_collect(mut self, enabled: bool) -> Self {
        self.auto_collect = enabled;
        self
    }
}
impl OutputSpec {
    /// Creates a single-value output specification with an ID derived from its
    /// name.
    ///
    /// Use a struct literal with an explicit [`SlotId`] when a stable identity
    /// must survive a slot rename or declaration reorder.
    pub fn new<T: Any>(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            id: slot_id(&name),
            name,
            value_type: SlotTypeId::of::<T>(),
        }
    }
}
impl Schema {
    /// Creates a schema from ordered input and output declarations.
    ///
    /// Local schema invariants are checked when the schema is added to or
    /// replaces a graph node.
    pub fn new(inputs: Vec<InputSpec>, outputs: Vec<OutputSpec>) -> Self {
        Self { inputs, outputs }
    }
    /// Starts incremental construction of an ordered schema.
    pub fn builder() -> SchemaBuilder {
        SchemaBuilder::default()
    }
}
/// Fluent builder for [`Schema`].
///
/// It preserves insertion order; [`build`](Self::build) does not perform graph
/// validation by itself.
#[derive(Default)]
pub struct SchemaBuilder {
    inputs: Vec<InputSpec>,
    outputs: Vec<OutputSpec>,
}
impl SchemaBuilder {
    /// Appends one input declaration.
    pub fn input(mut self, input: InputSpec) -> Self {
        self.inputs.push(input);
        self
    }
    /// Appends one output declaration.
    pub fn output(mut self, output: OutputSpec) -> Self {
        self.outputs.push(output);
        self
    }
    /// Finishes the builder without changing declaration order.
    pub fn build(self) -> Schema {
        Schema::new(self.inputs, self.outputs)
    }
}

fn slot_id(name: &str) -> SlotId {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in name.as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    SlotId(hash)
}
