//! Scenario 28: dispatch independent Send nodes through a host-owned pool.
//!
//! Three independent producers are explicitly collected into one `Many` input.
//! `ExecutionGraphVersion::execute_on` leaves graph readiness and output commit
//! with `GraphRun`, while this tiny host adapter schedules ready `NodeJob`s on
//! two worker threads. The public API is currently a skeleton, so running this
//! example deliberately reaches an unimplemented operation.

use futures_lite::future::block_on;
use slot_graph::{
    DispatchError, Graph, InputSpec, NodeDispatcher, NodeJob, NodeOutputs, OutputSpec, Schema,
    SendMode,
};
use std::{
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderItem(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawList(u32);

/// A deliberately small host adapter, not a recommended general-purpose pool.
///
/// `Receiver` is protected only while taking one job. A worker releases the
/// mutex before polling it, allowing the other worker to take another ready
/// node. Dropping the sender closes the queue; `shutdown` then joins workers.
struct TwoThreadPool {
    dispatcher: Option<PoolDispatcher>,
    workers: Vec<JoinHandle<()>>,
}

/// Cloneable dispatcher ownership passed into a single graph run.
#[derive(Clone)]
struct PoolDispatcher {
    // Rust 1.71 does not expose `mpsc::Sender<T>` as Sync for this use, so the
    // adapter supplies the synchronization required by a SendMode dispatcher.
    sender: Arc<Mutex<Sender<NodeJob<SendMode>>>>,
}

impl TwoThreadPool {
    fn new() -> Self {
        let (sender, receiver) = mpsc::channel::<NodeJob<SendMode>>();
        let receiver = Arc::new(Mutex::new(receiver));
        let mut workers = Vec::new();

        for _ in 0..2 {
            let receiver = Arc::clone(&receiver);
            workers.push(thread::spawn(move || worker_loop(receiver)));
        }

        Self {
            dispatcher: Some(PoolDispatcher {
                sender: Arc::new(Mutex::new(sender)),
            }),
            workers,
        }
    }

    fn dispatcher(&self) -> PoolDispatcher {
        self.dispatcher
            .as_ref()
            .expect("dispatcher requested after shutdown")
            .clone()
    }

    fn shutdown(&mut self) {
        // Closing the last sender wakes every receiver with `Disconnected`.
        self.dispatcher.take();
        for worker in self.workers.drain(..) {
            worker.join().expect("example worker must not panic");
        }
    }
}

impl Drop for TwoThreadPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(receiver: Arc<Mutex<Receiver<NodeJob<SendMode>>>>) {
    loop {
        let job = match receiver.lock().expect("receiver mutex poisoned").recv() {
            Ok(job) => job,
            Err(_) => return,
        };

        // The identifier is useful for host tracing. Completion is reported
        // back to GraphRun by NodeJob; this worker never mutates graph state.
        let _node = job.node_id();
        block_on(job);
    }
}

impl NodeDispatcher<SendMode> for PoolDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        self.sender
            .lock()
            .expect("sender mutex poisoned")
            .send(job)
            // `SendError<NodeJob>` cannot be retained as a thread-safe source:
            // NodeJob deliberately is not Sync. The host diagnostic is enough.
            .map_err(|_| DispatchError::new("external worker pool stopped"))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::<SendMode>::new();

    let producer_schema = Schema::builder()
        .output(OutputSpec::new::<RenderItem>("item"))
        .build();
    let first = graph.add_sync("first", producer_schema.clone(), |_, _| {
        let mut outputs = NodeOutputs::new();
        outputs.insert("item", RenderItem(10));
        Ok(outputs)
    })?;
    let second = graph.add_sync("second", producer_schema.clone(), |_, _| {
        let mut outputs = NodeOutputs::new();
        outputs.insert("item", RenderItem(20));
        Ok(outputs)
    })?;
    let third = graph.add_sync("third", producer_schema, |_, _| {
        let mut outputs = NodeOutputs::new();
        outputs.insert("item", RenderItem(30));
        Ok(outputs)
    })?;

    let join_schema = Schema::builder()
        .input(InputSpec::required_many::<RenderItem>("items"))
        .output(OutputSpec::new::<DrawList>("draws"))
        .build();
    let join = graph.add_sync("join", join_schema, move |_, inputs| {
        let total = inputs
            .many::<RenderItem>("items")?
            .iter()
            .map(|item| item.0)
            .sum();
        let mut outputs = NodeOutputs::new();
        outputs.insert("draws", DrawList(total));
        Ok(outputs)
    })?;

    // This establishes only `RenderItem -> join.items` edges in source order.
    // It neither scans the rest of the graph nor changes runtime semantics.
    let items = graph.input::<RenderItem>(join, "items")?;
    let edges = graph.collect_into([first, second, third], items)?;
    assert_eq!(edges.len(), 3);
    graph.set_active(join, true)?;

    let output = graph.output::<DrawList>(join, "draws")?;
    let version = graph.compile()?;
    let mut pool = TwoThreadPool::new();
    let mut report = block_on(version.execute_on(Default::default(), pool.dispatcher()))?;
    assert_eq!(*report.take_output(output)?, DrawList(60));

    // All jobs have completed before shutdown. This is required for a host
    // pool; it must outlive every GraphRun that dispatches into it.
    pool.shutdown();
    Ok(())
}
