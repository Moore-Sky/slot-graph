mod common;

use async_runtime::{Priority, RuntimeBuilder, Spawner};
use common::{criterion_config, IoStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures_lite::future::block_on;
use slot_graph::{DispatchError, NodeDispatcher, NodeJob, RunInputs, SendMode};
use std::num::NonZeroUsize;
use std::sync::{Arc, Condvar, Mutex};

/// Tracks accepted jobs through the end of their runtime task. A graph driver
/// can finish after consuming a completion event while the task wrapper still
/// has cleanup to perform, so timed iterations wait for quiescence.
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

/// The guard is owned by the submitted task, so spawn failure also decrements
/// the count when the rejected future is dropped.
struct Completion(Arc<InFlight>);

impl Drop for Completion {
    fn drop(&mut self) {
        self.0.finish();
    }
}

#[derive(Clone)]
struct AsyncRuntimeDispatcher {
    spawner: Spawner,
    in_flight: Arc<InFlight>,
}

impl NodeDispatcher<SendMode> for AsyncRuntimeDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        let completion = self.in_flight.begin();
        let task = self
            .spawner
            .spawn(Priority::Normal, async move {
                let _completion = completion;
                job.await;
            })
            .map_err(DispatchError::with_source)?;
        task.detach();
        Ok(())
    }
}

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("async_runtime_integration");
    for &workers in &[1_usize, 8] {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).unwrap())
            .build()
            .unwrap();
        let warmups: Vec<_> = (0..workers * 4)
            .map(|_| runtime.spawn(Priority::Normal, async {}).unwrap())
            .collect();
        for warmup in warmups {
            block_on(warmup);
        }
        let dispatcher = AsyncRuntimeDispatcher {
            spawner: runtime.spawner(),
            in_flight: Arc::new(InFlight::new()),
        };
        for (shape, graph, output, expected) in {
            let (chain, chain_output) = common::build_send_chain(100, IoStyle::Bound);
            let (wide, wide_output) = common::build_send_independent(100);
            let (pending, pending_output) =
                common::build_send_async_chain(100, common::AsyncStyle::PendingOnce);
            let (wake_storm, wake_storm_output) =
                common::build_send_async_independent(100, common::AsyncStyle::WakeStorm);
            let (mixed, mixed_output) =
                common::build_send_async_chain(100, common::AsyncStyle::Mixed);
            [
                ("sync_chain", chain, chain_output, 100),
                ("sync_independent", wide, wide_output, 99),
                ("async_pending_chain", pending, pending_output, 100),
                (
                    "async_wake_storm_independent",
                    wake_storm,
                    wake_storm_output,
                    99,
                ),
                ("mixed_80_sync_20_async", mixed, mixed_output, 100),
            ]
        } {
            let version = graph.compile().unwrap();
            let mut preflight =
                block_on(version.execute_on(RunInputs::new(), dispatcher.clone())).unwrap();
            assert_eq!(*preflight.take_output(output).unwrap(), expected);
            drop(preflight);
            dispatcher.in_flight.wait_idle();
            {
                let mut runner = version.runner_on(dispatcher.clone());
                group.bench_function(format!("{shape}/100/workers_{workers}"), |b| {
                    b.iter(|| {
                        let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                        black_box(*report.take_output(output).unwrap());
                        drop(report);
                        dispatcher.in_flight.wait_idle();
                    })
                });
            }
        }
        drop(dispatcher);
        runtime.shutdown_graceful().unwrap();
    }
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
