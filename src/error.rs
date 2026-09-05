//! Structured errors for editing, compilation, startup, tasks, reports, and execution.
//! Match error kinds and context, rather than parsing Display text.

use crate::{
    handles::{EdgeId, GraphId, NodeId, SlotId},
    mode::{Mode, UserErrorFor},
    report::RunReport,
};
use std::{error::Error, fmt, marker::PhantomData};

/// A task failure, with an optional application error retained as its source.
/// Local permits thread-local sources; SendMode requires Send + Sync sources.
pub struct NodeError<M: Mode> {
    /// Machine-readable failure category.
    pub kind: NodeErrorKind,
    /// Graph/slot context associated with the failure.
    pub context: Box<ErrorContext>,
    source: Option<M::UserError>,
    dispatch_source: Option<DispatchError>,
    _mode: PhantomData<M>,
}
/// Categories of task failures; new variants may be added in compatible releases.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeErrorKind {
    /// Application-defined error; available through Error::source.
    User,
    /// An unwind panic while invoking or polling a task.
    Panic,
    /// Input access used a wrong name, type, layout, or declared input shape.
    InvalidInputs,
    /// The returned output bag does not match the complete Schema.
    InvalidOutputs,
    /// An external node dispatcher rejected, lost, or panicked while accepting the job.
    Dispatch,
    /// The task observed a cancellation checkpoint.
    Cancelled,
    /// An invariant inside the library failed.
    InternalInvariantViolation,
}

/// Failure reported by an application-provided node dispatcher.
///
/// A dispatcher returns this only when it cannot accept a Ready node job. The
/// graph attaches the affected node identity and records a Dispatch node
/// failure; other independent branches may continue. This error is Send and
/// Sync so the same adapter can be used by a work-stealing Send executor.
pub struct DispatchError {
    message: String,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl DispatchError {
    /// Creates a rejection with a human-readable diagnostic and no source.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// Retains a thread-safe executor or queue error as the error source.
    pub fn with_source<E>(source: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        Self {
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Debug for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DispatchError")
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for DispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for DispatchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}
/// Available identity and name context for a structured error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ErrorContext {
    /// Owning graph, when known.
    pub graph: Option<GraphId>,
    /// Affected node, when known.
    pub node: Option<NodeId>,
    /// Affected edge, when known.
    pub edge: Option<EdgeId>,
    /// Affected Slot identity, when known.
    pub slot: Option<SlotId>,
    /// Relevant node or Slot name, when known.
    pub name: Option<String>,
}
macro_rules! error_type {
    ($name:ident, $kind:ident { $($(#[$meta:meta])* $variant:ident),+ $(,)? }) => {
        #[doc = concat!("Machine-readable categories for [`", stringify!($name), "`].")]
        #[derive(Clone, Debug, PartialEq, Eq)]
        #[non_exhaustive]
        pub enum $kind { $($(#[$meta])* $variant),+ }
        #[doc = concat!("A structured `", stringify!($name), "` with identity context.")]
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name {
            /// Machine-readable error classification.
            pub kind: $kind,
            /// Relevant identities and name for diagnostics.
            pub context: ErrorContext
        }
        impl $name {
            pub(crate) fn new(kind: $kind, context: ErrorContext) -> Self {
                Self { kind, context }
            }
        }
        impl fmt::Display for $name { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{:?}", self.kind) } }
        impl Error for $name {}
    };
}
error_type!(
    EditError,
    EditErrorKind {
        /// A node already uses the requested name.
        DuplicateNodeName,
        /// The requested node name is empty or invalid.
        InvalidNodeName,
        /// Slot names, identities, or declarations violate Schema rules.
        InvalidSchema,
        /// No node resolves to this name.
        UnknownNodeName,
        /// No Slot resolves to this name.
        UnknownSlotName,
        /// The node identity has been removed or superseded.
        StaleNodeId,
        /// The handle does not belong to the applicable Schema generation.
        StaleSlotHandle,
        /// The edge identity has been removed.
        StaleEdgeId,
        /// The handle belongs to another graph.
        ForeignHandle,
        /// The requested connection uses the wrong Slot direction.
        WrongDirection,
        /// The value types are not exactly compatible.
        TypeMismatch,
        /// The input would have more sources than allowed.
        CardinalityOverflow,
        /// Explicit bulk collection requires a Many input, not a One input.
        ExpectedManyInput,
        /// The same output/input connection already exists.
        DuplicateEdge,
        /// An input cannot combine exposed and ordinary producers.
        InputSourceConflict,
        /// Automatic matching has multiple equally eligible candidates.
        AmbiguousAutoMatch
    }
);
error_type!(
    CompileError,
    CompileErrorKind {
        /// A selected node has an unsatisfied Required input.
        MissingRequiredInput,
        /// No execution target is selected.
        NoActiveTarget,
        /// The selected dependency closure contains a cycle.
        CycleDetected,
        /// A selected binding violates the compiled contract.
        InvalidBinding
    }
);
error_type!(
    StartError,
    StartErrorKind {
        /// An exposed Required value is absent.
        MissingRunInput,
        /// The same exposed input was supplied more than once.
        DuplicateRunInput,
        /// An input key is not accepted by this version.
        UnexpectedRunInput,
        /// The supplied input count violates its cardinality.
        RunInputCardinality,
        /// An external value has an incompatible type.
        RunInputTypeMismatch
    }
);
error_type!(
    OutputAccessError,
    OutputAccessErrorKind {
        /// The output was not an Active target output.
        NotCollected,
        /// The target did not successfully commit its outputs.
        OutputUnavailable,
        /// The output was already moved out of the report.
        OutputTaken,
        /// The handle belongs to another graph.
        ForeignHandle,
        /// The handle does not belong to the applicable Schema generation.
        StaleSlotHandle,
        /// The value types are not exactly compatible.
        TypeMismatch
    }
);
impl<M: Mode> fmt::Debug for NodeError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeError")
            .field("kind", &self.kind)
            .field("context", &self.context)
            .finish()
    }
}
impl<M: Mode> fmt::Display for NodeError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.kind)
    }
}
impl<M: Mode> Error for NodeError<M> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_ref().map(M::user_error_ref).or_else(|| {
            self.dispatch_source
                .as_ref()
                .map(|error| error as &dyn Error)
        })
    }
}
#[derive(Debug)]
struct MessageError(String);
impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for MessageError {}
impl<M: Mode> NodeError<M> {
    pub(crate) fn internal(kind: NodeErrorKind, context: ErrorContext) -> Self {
        Self {
            kind,
            context: Box::new(context),
            source: None,
            dispatch_source: None,
            _mode: PhantomData,
        }
    }

    pub(crate) fn dispatch(source: DispatchError, context: ErrorContext) -> Self {
        Self {
            kind: NodeErrorKind::Dispatch,
            context: Box::new(context),
            source: None,
            dispatch_source: Some(source),
            _mode: PhantomData,
        }
    }

    /// Retains an application failure without erasing its Error::source chain.
    /// The source must meet the selected mode's thread-safety requirements.
    pub fn user<E: UserErrorFor<M>>(source: E) -> Self {
        Self {
            kind: NodeErrorKind::User,
            context: Box::default(),
            source: Some(source.into_user_error()),
            dispatch_source: None,
            _mode: PhantomData,
        }
    }
}
impl<M: Mode> From<String> for NodeError<M>
where
    MessageError: UserErrorFor<M>,
{
    fn from(value: String) -> Self {
        Self::user(MessageError(value))
    }
}
impl<M: Mode> From<&str> for NodeError<M>
where
    MessageError: UserErrorFor<M>,
{
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}
/// Failure to start, or a completed run report containing failures/cancellation.
#[non_exhaustive]
pub enum ExecuteError<M: Mode> {
    /// Input validation failed before any task started.
    Start(StartError),
    /// Independent runnable branches finished; at least one task failed.
    Failed(RunReport<M>),
    /// Cancellation completed; previously committed target outputs remain available.
    Cancelled(RunReport<M>),
}
impl<M: Mode> fmt::Debug for ExecuteError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Start(e) => f.debug_tuple("Start").field(e).finish(),
            Self::Failed(_) => f.write_str("Failed(RunReport)"),
            Self::Cancelled(_) => f.write_str("Cancelled(RunReport)"),
        }
    }
}
impl<M: Mode> fmt::Display for ExecuteError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("slot-graph execution failed")
    }
}
impl<M: Mode> Error for ExecuteError<M> {}
