//! Immutable compiled versions that can start isolated runs or reusable runners.
//!
//! Send execution rejects a dispatcher with thread-local state:
//! ```compile_fail
//! use slot_graph::{DispatchError, Graph, NodeJob, RunInputs, SendMode};
//! use std::rc::Rc;
//! let version = Graph::<SendMode>::new().compile().unwrap();
//! let state = Rc::new(());
//! let dispatcher = move |_job: NodeJob<SendMode>| {
//!     let _ = &state;
//!     Ok::<(), DispatchError>(())
//! };
//! let _run = version.execute_on(RunInputs::new(), dispatcher);
//! ```

use crate::{
    error::{CompileError, CompileErrorKind, ErrorContext, StartError},
    graph::Graph,
    handles::{GraphId, NodeId, SlotTypeId, VersionId},
    mode::{Local, Mode, SendMode},
    runtime::{Dispatcher, GraphRun, GraphRunner, NodeDispatcher, RunInputs},
    schema::{BoundSchema, Cardinality, Presence},
    task::Task,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    marker::PhantomData,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

static NEXT_VERSION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct CompiledPlan<M: Mode> {
    pub(crate) graph: GraphId,
    pub(crate) version: VersionId,
    pub(crate) nodes: Vec<CompiledNode<M>>,
    pub(crate) node_index: HashMap<NodeId, usize>,
}

pub(crate) struct CompiledNode<M: Mode> {
    pub(crate) id: NodeId,
    pub(crate) name: String,
    pub(crate) schema_generation: u64,
    pub(crate) schema: BoundSchema,
    pub(crate) task: Task<M>,
    pub(crate) inputs: Vec<CompiledInput>,
    pub(crate) predecessors: Vec<usize>,
    pub(crate) successors: Vec<usize>,
    pub(crate) active: bool,
}

pub(crate) struct CompiledInput {
    pub(crate) sources: Vec<CompiledSource>,
    pub(crate) external: Option<CompiledExternal>,
}

pub(crate) struct CompiledSource {
    pub(crate) node: usize,
    pub(crate) output: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct CompiledExternal {
    pub(crate) binding: u64,
    pub(crate) value_type: SlotTypeId,
    pub(crate) presence: Presence,
    pub(crate) cardinality: Cardinality,
}

/// Immutable plan produced by compiling a declaration graph.
pub struct ExecutionGraphVersion<M: Mode> {
    graph: GraphId,
    pub(crate) plan: Arc<CompiledPlan<M>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> ExecutionGraphVersion<M> {
    /// Identifies this immutable compilation independently of other versions.
    pub fn version_id(&self) -> VersionId {
        self.plan.version
    }
    /// Returns the declaration graph identity captured by this version.
    pub fn graph_id(&self) -> GraphId {
        self.graph
    }
    /// Validates inputs and creates an owned run.
    pub fn start(&self, _inputs: RunInputs<M>) -> Result<GraphRun<M>, StartError> {
        GraphRun::start_inline(Arc::clone(&self.plan), _inputs)
    }
    /// Creates an execution future which returns start or execution errors.
    pub fn execute(&self, _inputs: RunInputs<M>) -> GraphRun<M> {
        GraphRun::execute_inline(Arc::clone(&self.plan), _inputs)
    }
    /// Creates independent reusable runner storage.
    pub fn runner(&self) -> GraphRunner<M> {
        GraphRunner::new(self.clone())
    }
}

impl<M: Mode> Clone for ExecutionGraphVersion<M> {
    fn clone(&self) -> Self {
        Self {
            graph: self.graph,
            plan: Arc::clone(&self.plan),
            _mode: PhantomData,
        }
    }
}

pub(crate) fn compile_graph<M: Mode>(
    graph: &Graph<M>,
) -> Result<ExecutionGraphVersion<M>, CompileError> {
    let active: Vec<NodeId> = graph
        .nodes
        .iter()
        .flatten()
        .filter(|node| node.active)
        .map(|node| node.id)
        .collect();
    if active.is_empty() {
        return Err(compile_error(
            graph.graph_id(),
            CompileErrorKind::NoActiveTarget,
            None,
        ));
    }

    let mut selected = HashSet::new();
    let mut stack = active;
    while let Some(node) = stack.pop() {
        if !selected.insert(node) {
            continue;
        }
        for edge in graph
            .edges
            .iter()
            .flatten()
            .filter(|edge| edge.target_node == node)
        {
            stack.push(edge.source_node);
        }
    }

    for node in graph
        .nodes
        .iter()
        .flatten()
        .filter(|node| selected.contains(&node.id))
    {
        for input in &node.schema.schema().inputs {
            let connected = graph
                .edges
                .iter()
                .flatten()
                .any(|edge| edge.target_node == node.id && edge.input == input.id);
            let exposed = graph
                .exposed
                .iter()
                .any(|binding| binding.node == node.id && binding.input == input.id);
            if input.presence == Presence::Required && !connected && !exposed {
                return Err(compile_error(
                    graph.graph_id(),
                    CompileErrorKind::MissingRequiredInput,
                    Some(node.id),
                ));
            }
        }
    }

    let selected_ids: Vec<NodeId> = graph
        .nodes
        .iter()
        .flatten()
        .filter(|node| selected.contains(&node.id))
        .map(|node| node.id)
        .collect();
    let node_index: HashMap<NodeId, usize> = selected_ids
        .iter()
        .copied()
        .enumerate()
        .map(|(index, node)| (node, index))
        .collect();
    let mut predecessors = vec![Vec::<usize>::new(); selected_ids.len()];
    let mut successors = vec![Vec::<usize>::new(); selected_ids.len()];
    for edge in graph.edges.iter().flatten() {
        let (Some(&source), Some(&target)) = (
            node_index.get(&edge.source_node),
            node_index.get(&edge.target_node),
        ) else {
            continue;
        };
        if !predecessors[target].contains(&source) {
            predecessors[target].push(source);
            successors[source].push(target);
        }
    }
    let mut remaining: Vec<usize> = predecessors.iter().map(Vec::len).collect();
    let mut ready: VecDeque<usize> = remaining
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect();
    let mut visited = 0;
    while let Some(node) = ready.pop_front() {
        visited += 1;
        for &successor in &successors[node] {
            remaining[successor] -= 1;
            if remaining[successor] == 0 {
                ready.push_back(successor);
            }
        }
    }
    if visited != selected_ids.len() {
        return Err(compile_error(
            graph.graph_id(),
            CompileErrorKind::CycleDetected,
            None,
        ));
    }

    let mut nodes = Vec::with_capacity(selected_ids.len());
    for (dense, id) in selected_ids.iter().copied().enumerate() {
        let node = graph.node(id).expect("selected node remains live");
        let inputs = node
            .schema
            .schema()
            .inputs
            .iter()
            .map(|input| {
                let sources = graph
                    .edges
                    .iter()
                    .flatten()
                    .filter(|edge| edge.target_node == id && edge.input == input.id)
                    .map(|edge| {
                        let source = node_index[&edge.source_node];
                        let output = graph
                            .node(edge.source_node)
                            .expect("edge source remains live")
                            .schema
                            .schema()
                            .outputs
                            .iter()
                            .position(|output| output.id == edge.output)
                            .expect("edge output remains live");
                        CompiledSource {
                            node: source,
                            output,
                        }
                    })
                    .collect();
                let external = graph
                    .exposed
                    .iter()
                    .find(|binding| binding.node == id && binding.input == input.id)
                    .map(|binding| CompiledExternal {
                        binding: binding.binding,
                        value_type: binding.value_type,
                        presence: binding.presence,
                        cardinality: binding.cardinality,
                    });
                CompiledInput { sources, external }
            })
            .collect();
        nodes.push(CompiledNode {
            id,
            name: node.name.clone(),
            schema_generation: node.schema_generation,
            schema: node.schema.clone(),
            task: node.task.clone(),
            inputs,
            predecessors: predecessors[dense].clone(),
            successors: successors[dense].clone(),
            active: node.active,
        });
    }
    let version = VersionId(NEXT_VERSION_ID.fetch_add(1, Ordering::Relaxed));
    let plan = Arc::new(CompiledPlan {
        graph: graph.graph_id(),
        version,
        nodes,
        node_index,
    });
    Ok(ExecutionGraphVersion {
        graph: graph.graph_id(),
        plan,
        _mode: PhantomData,
    })
}

fn compile_error(graph: GraphId, kind: CompileErrorKind, node: Option<NodeId>) -> CompileError {
    CompileError::new(
        kind,
        ErrorContext {
            graph: Some(graph),
            node,
            ..ErrorContext::default()
        },
    )
}

impl ExecutionGraphVersion<Local> {
    /// Validates inputs and starts a run whose Ready nodes use a local dispatcher.
    ///
    /// The dispatcher and NodeJob may be !Send, but the dispatcher is owned and
    /// 'static. For example, an async-runtime adapter can own `Rc<LocalDomain>`
    /// while another `Rc` clone drives that domain on its owner thread. A borrowed
    /// `&LocalDomain` adapter is not accepted.
    pub fn start_on<D>(
        &self,
        inputs: RunInputs<Local>,
        dispatcher: D,
    ) -> Result<GraphRun<Local>, StartError>
    where
        D: NodeDispatcher<Local>,
    {
        GraphRun::start_dispatched(Arc::clone(&self.plan), inputs, Dispatcher::new(dispatcher))
    }

    /// Creates a local dispatcher-backed execution future. Start failures are
    /// returned by the future like ordinary execute.
    pub fn execute_on<D>(&self, inputs: RunInputs<Local>, dispatcher: D) -> GraphRun<Local>
    where
        D: NodeDispatcher<Local>,
    {
        GraphRun::execute_dispatched(Arc::clone(&self.plan), inputs, Dispatcher::new(dispatcher))
    }

    /// Creates reusable local run storage using the supplied owner-thread dispatcher.
    pub fn runner_on<D>(&self, dispatcher: D) -> GraphRunner<Local>
    where
        D: NodeDispatcher<Local>,
    {
        GraphRunner::new_on(self.clone(), Dispatcher::new(dispatcher))
    }
}

impl ExecutionGraphVersion<SendMode> {
    /// Validates inputs and starts a run whose Ready nodes may execute in parallel.
    ///
    /// The dispatcher is executor-neutral. Send + Sync is an intentionally
    /// conservative adapter contract even though one GraphRun is never polled
    /// concurrently. Each submitted NodeJob is Send.
    pub fn start_on<D>(
        &self,
        inputs: RunInputs<SendMode>,
        dispatcher: D,
    ) -> Result<GraphRun<SendMode>, StartError>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        GraphRun::start_dispatched(Arc::clone(&self.plan), inputs, Dispatcher::new(dispatcher))
    }

    /// Creates a Send dispatcher-backed execution future. Spawning this
    /// GraphRun schedules orchestration; independently Ready NodeJobs are still
    /// submitted separately to the dispatcher.
    pub fn execute_on<D>(&self, inputs: RunInputs<SendMode>, dispatcher: D) -> GraphRun<SendMode>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        GraphRun::execute_dispatched(Arc::clone(&self.plan), inputs, Dispatcher::new(dispatcher))
    }

    /// Creates reusable Send run storage using the supplied external dispatcher.
    pub fn runner_on<D>(&self, dispatcher: D) -> GraphRunner<SendMode>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        GraphRunner::new_on(self.clone(), Dispatcher::new(dispatcher))
    }
}
