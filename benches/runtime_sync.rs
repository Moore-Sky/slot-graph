mod common;

use common::{criterion_config, IoStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures_lite::future::block_on;
use slot_graph::{
    DispatchError, Graph, Local, NodeDispatcher, NodeJob, NodeOutputs, OutputSlot, RunInputs,
    SendMode,
};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

type JobEnvelope = (NodeJob<SendMode>, Completion);

/// Minimal fixed worker pool used only to contrast inline execution with
/// external Ready-node dispatch. The dedicated `dispatch` benchmark retains
/// the broader adapter matrix; this local copy keeps diagnostic D comparable.
#[derive(Clone)]
struct PoolDispatcher {
    sender: Arc<Mutex<mpsc::Sender<JobEnvelope>>>,
    in_flight: Arc<InFlight>,
}

struct InFlight {
    count: Mutex<usize>,
    idle: Condvar,
}

impl InFlight {
    fn new() -> Self {
        Self {
            count: Mutex::new(0),
            idle: Condvar::new(),
        }
    }

    fn begin(self: &Arc<Self>) -> Completion {
        *self.count.lock().unwrap() += 1;
        Completion(Arc::clone(self))
    }

    fn finish(&self) {
        let mut count = self.count.lock().unwrap();
        *count -= 1;
        if *count == 0 {
            self.idle.notify_all();
        }
    }

    fn wait_idle(&self) {
        let mut count = self.count.lock().unwrap();
        while *count != 0 {
            count = self.idle.wait(count).unwrap();
        }
    }
}

/// Keeps accounting correct if a worker unwinds or enqueueing fails.
struct Completion(Arc<InFlight>);

impl Drop for Completion {
    fn drop(&mut self) {
        self.0.finish();
    }
}

impl NodeDispatcher<SendMode> for PoolDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        let completion = self.in_flight.begin();
        self.sender
            .lock()
            .unwrap()
            .send((job, completion))
            .map_err(|_| DispatchError::new("pool stopped"))
    }
}

struct Pool {
    dispatcher: Option<PoolDispatcher>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    fn new(worker_count: usize) -> Self {
        let (sender, receiver) = mpsc::channel::<JobEnvelope>();
        let receiver = Arc::new(Mutex::new(receiver));
        let workers = (0..worker_count)
            .map(|_| {
                let receiver = Arc::clone(&receiver);
                thread::spawn(move || loop {
                    let (job, _completion) = match receiver.lock().unwrap().recv() {
                        Ok(envelope) => envelope,
                        Err(_) => break,
                    };
                    block_on(job);
                })
            })
            .collect();
        Self {
            dispatcher: Some(PoolDispatcher {
                sender: Arc::new(Mutex::new(sender)),
                in_flight: Arc::new(InFlight::new()),
            }),
            workers,
        }
    }

    fn dispatcher(&self) -> PoolDispatcher {
        self.dispatcher.as_ref().unwrap().clone()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.dispatcher.take();
        for worker in self.workers.drain(..) {
            worker.join().unwrap();
        }
    }
}

fn build_local_named_renderer_frame(
    width: usize,
    layers: usize,
) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(width > 0 && layers > 0);
    let mut graph = Graph::<Local>::new();
    let mut previous = Vec::with_capacity(width);
    for index in 0..width {
        previous.push(
            graph
                .add_sync(
                    format!("layer_0_node_{index}"),
                    common::source_schema(),
                    move |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", index as u64 + 1);
                        Ok(outputs)
                    },
                )
                .unwrap(),
        );
    }

    for layer in 1..layers {
        let mut current = Vec::with_capacity(width);
        for index in 0..width {
            let node = graph
                .add_sync(
                    format!("layer_{layer}_node_{index}"),
                    common::many_schema(),
                    move |_, inputs| {
                        let value = inputs
                            .many::<u64>("values")?
                            .iter()
                            .fold(index as u64, |sum, value| sum.wrapping_add(**value));
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", value);
                        Ok(outputs)
                    },
                )
                .unwrap();
            let target = graph.input::<u64>(node, "values").unwrap();
            graph
                .collect_into(previous.iter().copied(), target)
                .unwrap();
            current.push(node);
        }
        previous = current;
    }

    for &node in &previous {
        graph.set_active(node, true).unwrap();
    }
    let output = graph
        .output::<u64>(*previous.last().unwrap(), "value")
        .unwrap();
    (graph, output)
}

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_sync");

    // Diagnostic A: independent active no-ops isolate GraphRun lifecycle and
    // scheduler bookkeeping from slot delivery and task work.
    let graph = common::build_local_active_empty(100);
    let version = graph.compile().unwrap();
    let preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(preflight.failures().len(), 0);
    group.bench_function("diagnostic_a/active_empty/fresh/100", |b| {
        b.iter(|| {
            let report = block_on(version.execute(RunInputs::new())).unwrap();
            black_box(report.failures().len());
        });
    });
    let mut runner = version.runner();
    group.bench_function("diagnostic_a/active_empty/reusable/100", |b| {
        b.iter(|| {
            let report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(report.failures().len());
        });
    });

    // Diagnostic B: one hundred scalar deliveries expose the incremental
    // cost of StoredValue, NodeInputs, NodeOutputs, and edge readiness.
    let (graph, output) = common::build_local_chain(100, 100, IoStyle::Bound);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), 100);
    group.bench_function("diagnostic_b/scalar_chain/fresh/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(version.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    let mut runner = version.runner();
    group.bench_function("diagnostic_b/scalar_chain/reusable/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });

    // Diagnostic C: the same node count as A/B, but each layer fan-outs to a
    // Many input on every next-layer node. It isolates edge delivery, shared
    // value cloning, and Many collection from the scalar-chain baseline.
    let (graph, output) = common::build_local_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    group.bench_function("diagnostic_c/fanout_fanin_many/fresh/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(version.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    let mut runner = version.runner();
    group.bench_function("diagnostic_c/fanout_fanin_many/reusable/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });

    // Diagnostic D: identical independent Send nodes, first run inline then
    // through fixed worker pools. This separates DAG orchestration from the
    // fixed adapter dispatch overhead measured in the dedicated suite.
    let (graph, output) = common::build_send_independent(100);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), 99);
    let mut runner = version.runner();
    group.bench_function(
        "diagnostic_d/independent_ready_send/inline/reusable/100",
        |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        },
    );
    for workers in [1_usize, 8] {
        let pool = Pool::new(workers);
        let dispatcher = pool.dispatcher();
        let mut preflight =
            block_on(version.execute_on(RunInputs::new(), dispatcher.clone())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 99);
        drop(preflight);
        dispatcher.in_flight.wait_idle();
        let mut runner = version.runner_on(dispatcher.clone());
        group.bench_function(
            format!(
                "diagnostic_d/independent_ready_send/mutex_mpsc_workers_{workers}/reusable/100"
            ),
            |b| {
                b.iter(|| {
                    let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                    black_box(*report.take_output(output).unwrap());
                    drop(report);
                    dispatcher.in_flight.wait_idle();
                });
            },
        );
    }

    for io in [IoStyle::Named, IoStyle::Bound] {
        let label = match io {
            IoStyle::Named => "named",
            IoStyle::Bound => "bound",
        };
        let (graph, output) = common::build_local_chain(100, 100, io);
        let version = graph.compile().unwrap();
        let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 100);
        group.bench_function(format!("local/fresh/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(version.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
        let mut runner = version.runner();
        group.bench_function(format!("local/reusable/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
    }
    for io in [IoStyle::Named, IoStyle::Bound] {
        let label = match io {
            IoStyle::Named => "named",
            IoStyle::Bound => "bound",
        };
        let (graph, output) = common::build_send_chain(100, io);
        let version = graph.compile().unwrap();
        let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 100);
        group.bench_function(format!("send/fresh/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(version.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
        let mut runner = version.runner();
        group.bench_function(format!("send/reusable/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
    }
    let (graph, output) = common::build_local_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    let mut runner = version.runner();
    group.bench_function("local/reusable/renderer_frame/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    let (graph, output) = build_local_named_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    let mut runner = version.runner();
    group.bench_function("local/reusable/renderer_frame/named/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
