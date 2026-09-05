mod common;

use common::{criterion_config, IoStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures_lite::future::block_on;
use slot_graph::{DispatchError, NodeDispatcher, NodeJob, RunInputs, SendMode};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

type JobEnvelope = (NodeJob<SendMode>, Completion);

#[derive(Clone)]
struct PoolDispatcher {
    sender: Arc<Mutex<mpsc::Sender<JobEnvelope>>>,
    in_flight: Arc<InFlight>,
}

/// Tracks accepted jobs through the end of their worker wrapper. A completed
/// graph driver can observe its final event before that wrapper has returned,
/// so Criterion iterations must wait for this counter to reach zero.
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

/// Finishes the accounting even when enqueueing fails or a worker unwinds.
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
        // std::sync::mpsc has one Receiver, so dequeue is serialized by this
        // mutex. Workers release it before polling and can execute jobs in
        // parallel; benchmark names expose the adapter bottleneck explicitly.
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

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("dispatch");
    for &workers in &[1_usize, 8] {
        let pool = Pool::new(workers);
        for (shape, graph, output) in {
            let (chain, chain_output) = common::build_send_chain(100, IoStyle::Bound);
            let (wide, wide_output) = common::build_send_independent(100);
            [
                ("chain", chain, chain_output),
                ("independent", wide, wide_output),
            ]
        } {
            let version = graph.compile().unwrap();
            let dispatcher = pool.dispatcher();
            let mut preflight =
                block_on(version.execute_on(RunInputs::new(), dispatcher.clone())).unwrap();
            let expected = if shape == "chain" { 100 } else { 99 };
            assert_eq!(*preflight.take_output(output).unwrap(), expected);
            drop(preflight);
            dispatcher.in_flight.wait_idle();
            let mut runner = version.runner_on(dispatcher.clone());
            group.bench_function(
                format!("mutex_mpsc_dequeue/{shape}/100/workers_{workers}"),
                |b| {
                    b.iter(|| {
                        let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                        black_box(*report.take_output(output).unwrap());
                        drop(report);
                        dispatcher.in_flight.wait_idle();
                    })
                },
            );
        }
    }
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
