mod common;

use async_runtime::{Priority, RuntimeBuilder, Spawner};
use futures_lite::future::block_on;
use slot_graph::{DispatchError, NodeDispatcher, NodeJob, RunInputs, SendMode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::thread::{self, JoinHandle};

type JobEnvelope = (NodeJob<SendMode>, Completion);

struct CountingAllocator;

static ENABLED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);
static LIVE_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK_LIVE_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this forwards the exact allocation request to System.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            let live =
                LIVE_BYTES.fetch_add(layout.size() as u64, Ordering::SeqCst) + layout.size() as u64;
            if ENABLED.load(Ordering::SeqCst) {
                CALLS.fetch_add(1, Ordering::SeqCst);
                REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::SeqCst);
                update_peak(live);
            }
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE_BYTES.fetch_sub(layout.size() as u64, Ordering::SeqCst);
        // SAFETY: pointer and layout came from this System-backed allocator.
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: this forwards the original pointer/layout and requested size.
        let replacement = unsafe { System.realloc(pointer, layout, new_size) };
        if !replacement.is_null() {
            let old_size = layout.size() as u64;
            let new_size = new_size as u64;
            let live = if new_size >= old_size {
                LIVE_BYTES.fetch_add(new_size - old_size, Ordering::SeqCst) + new_size - old_size
            } else {
                LIVE_BYTES.fetch_sub(old_size - new_size, Ordering::SeqCst) - old_size + new_size
            };
            if ENABLED.load(Ordering::SeqCst) {
                CALLS.fetch_add(1, Ordering::SeqCst);
                // Realloc contributes its complete new request, not only growth.
                REQUESTED_BYTES.fetch_add(new_size, Ordering::SeqCst);
                update_peak(live);
            }
        }
        replacement
    }
}

fn update_peak(live: u64) {
    let mut peak = PEAK_LIVE_BYTES.load(Ordering::SeqCst);
    while live > peak {
        match PEAK_LIVE_BYTES.compare_exchange_weak(peak, live, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

#[derive(Default)]
struct Sample {
    calls: u64,
    requested_bytes: u64,
    peak_live_delta: u64,
    end_live_delta: i128,
}

fn measure(operation: impl FnOnce()) -> Sample {
    let baseline_live = LIVE_BYTES.load(Ordering::SeqCst);
    CALLS.store(0, Ordering::SeqCst);
    REQUESTED_BYTES.store(0, Ordering::SeqCst);
    PEAK_LIVE_BYTES.store(baseline_live, Ordering::SeqCst);
    ENABLED.store(true, Ordering::SeqCst);
    operation();
    ENABLED.store(false, Ordering::SeqCst);
    let end_live = LIVE_BYTES.load(Ordering::SeqCst);
    Sample {
        calls: CALLS.load(Ordering::SeqCst),
        requested_bytes: REQUESTED_BYTES.load(Ordering::SeqCst),
        peak_live_delta: PEAK_LIVE_BYTES
            .load(Ordering::SeqCst)
            .saturating_sub(baseline_live),
        end_live_delta: i128::from(end_live) - i128::from(baseline_live),
    }
}

fn print_average(name: &str, iterations: u64, samples: impl Iterator<Item = Sample>) {
    let total = samples.fold(Sample::default(), |mut total, sample| {
        total.calls += sample.calls;
        total.requested_bytes += sample.requested_bytes;
        total.peak_live_delta += sample.peak_live_delta;
        total.end_live_delta += sample.end_live_delta;
        total
    });
    println!(
        "{name},{},{},{},{},{}",
        total.calls / iterations,
        total.requested_bytes / iterations,
        total.peak_live_delta / iterations,
        total.end_live_delta / i128::from(iterations),
        iterations
    );
}

fn average(name: &str, iterations: u64, mut operation: impl FnMut()) {
    for _ in 0..10 {
        operation();
    }
    print_average(
        name,
        iterations,
        (0..iterations).map(|_| measure(&mut operation)),
    );
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

    fn begin(&self) {
        *self.count.lock().unwrap() += 1;
    }

    fn finish(&self) {
        let mut count = self.count.lock().unwrap();
        *count -= 1;
        if *count == 0 {
            self.idle.notify_all();
        }
    }

    fn wait(&self) {
        let mut count = self.count.lock().unwrap();
        while *count != 0 {
            count = self.idle.wait(count).unwrap();
        }
    }
}

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

impl AsyncRuntimeDispatcher {
    fn wait_idle(&self) {
        self.in_flight.wait();
    }
}

impl NodeDispatcher<SendMode> for AsyncRuntimeDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        self.in_flight.begin();
        let completion = Completion(Arc::clone(&self.in_flight));
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

/// A fixed worker-pool adapter matching `benches/dispatch.rs`. It is measured
/// separately from the async-runtime adapter so its queueing allocations stay
/// visible in the allocation baseline.
#[derive(Clone)]
struct PoolDispatcher {
    sender: Arc<Mutex<mpsc::Sender<JobEnvelope>>>,
    in_flight: Arc<InFlight>,
}

impl PoolDispatcher {
    fn wait_idle(&self) {
        self.in_flight.wait();
    }
}

impl NodeDispatcher<SendMode> for PoolDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        self.in_flight.begin();
        let completion = Completion(Arc::clone(&self.in_flight));
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
        // Keep this adapter intentionally identical to the dispatch benchmark:
        // dequeue is serialized, while worker polling remains parallel.
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

fn average_fixed_pool(name: &str, workers: usize, independent: bool, iterations: u64) {
    let pool = Pool::new(workers);
    let dispatcher = pool.dispatcher();
    let (graph, output, expected) = if independent {
        let (graph, output) = common::build_send_independent(100);
        (graph, output, 99)
    } else {
        let (graph, output) = common::build_send_chain(100, common::IoStyle::Bound);
        (graph, output, 100)
    };
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute_on(RunInputs::new(), dispatcher.clone())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), expected);
    dispatcher.wait_idle();
    drop(preflight);
    let mut runner = version.runner_on(dispatcher.clone());
    average(name, iterations, || {
        let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
        black_box(*report.take_output(output).unwrap());
        drop(report);
        dispatcher.wait_idle();
    });
    drop(runner);
    drop(dispatcher);
    drop(pool);
}

fn average_dispatched(
    name: &str,
    workers: usize,
    independent: bool,
    async_style: Option<common::AsyncStyle>,
    iterations: u64,
) {
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
    let (graph, output, expected) = match (independent, async_style) {
        (true, Some(style)) => {
            let (graph, output) = common::build_send_async_independent(100, style);
            (graph, output, 99)
        }
        (false, Some(style)) => {
            let (graph, output) = common::build_send_async_chain(100, style);
            (graph, output, 100)
        }
        (true, None) => {
            let (graph, output) = common::build_send_independent(100);
            (graph, output, 99)
        }
        (false, None) => {
            let (graph, output) = common::build_send_chain(100, common::IoStyle::Bound);
            (graph, output, 100)
        }
    };
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute_on(RunInputs::new(), dispatcher.clone())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), expected);
    dispatcher.wait_idle();
    drop(preflight);
    let mut runner = version.runner_on(dispatcher.clone());
    average(name, iterations, || {
        let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
        black_box(*report.take_output(output).unwrap());
        drop(report);
        dispatcher.wait_idle();
    });
    drop(runner);
    drop(dispatcher);
    runtime.shutdown_graceful().unwrap();
}

fn main() {
    const ITERATIONS: u64 = 100;
    println!(
        "workload,allocation_calls,gross_requested_bytes,peak_live_delta_bytes,\
         end_live_delta_bytes,iterations"
    );

    average("graph_build/chain/100", ITERATIONS, || {
        black_box(common::build_local_chain(100, 100, common::IoStyle::Bound));
    });

    let (compile_graph, _) = common::build_local_chain(100, 100, common::IoStyle::Bound);
    average("compile/full/100", ITERATIONS, || {
        black_box(compile_graph.compile().unwrap());
    });

    for io in [common::IoStyle::Named, common::IoStyle::Bound] {
        let label = match io {
            common::IoStyle::Named => "named",
            common::IoStyle::Bound => "bound",
        };
        let (graph, output) = common::build_local_chain(100, 100, io);
        let version = graph.compile().unwrap();
        let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 100);
        let mut runner = version.runner();
        average(
            &format!("runtime_sync/local/reusable/{label}/100"),
            ITERATIONS,
            || {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            },
        );
    }

    let (graph, output) = common::build_local_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    let mut runner = version.runner();
    average("runtime_sync/local/renderer_frame/100", ITERATIONS, || {
        let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
        black_box(*report.take_output(output).unwrap());
    });

    let (graph, output) = common::build_local_async_chain(100, common::AsyncStyle::PendingOnce);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), 100);
    let mut runner = version.runner();
    average(
        "runtime_async/local/reusable/pending_once/100",
        ITERATIONS,
        || {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        },
    );

    let (graph, output) = common::build_local_async_chain(100, common::AsyncStyle::Mixed);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(*preflight.take_output(output).unwrap(), 100);
    let mut runner = version.runner();
    average(
        "runtime_async/local/reusable/mixed_80_sync_20_async/100",
        ITERATIONS,
        || {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        },
    );

    for &workers in &[1_usize, 8] {
        average_fixed_pool(
            &format!("dispatch/mutex_mpsc_dequeue/chain/100/workers_{workers}"),
            workers,
            false,
            ITERATIONS,
        );
        average_fixed_pool(
            &format!("dispatch/mutex_mpsc_dequeue/independent/100/workers_{workers}"),
            workers,
            true,
            ITERATIONS,
        );
    }

    for &workers in &[1_usize, 8] {
        average_dispatched(
            &format!("async_runtime/sync_chain/100/workers_{workers}"),
            workers,
            false,
            None,
            ITERATIONS,
        );
        average_dispatched(
            &format!("async_runtime/sync_independent/100/workers_{workers}"),
            workers,
            true,
            None,
            ITERATIONS,
        );
        average_dispatched(
            &format!("async_runtime/async_pending_chain/100/workers_{workers}"),
            workers,
            false,
            Some(common::AsyncStyle::PendingOnce),
            ITERATIONS,
        );
        average_dispatched(
            &format!("async_runtime/async_wake_storm_wide/100/workers_{workers}"),
            workers,
            true,
            Some(common::AsyncStyle::WakeStorm),
            ITERATIONS,
        );
        average_dispatched(
            &format!("async_runtime/mixed_80_sync_20_async/100/workers_{workers}"),
            workers,
            false,
            Some(common::AsyncStyle::Mixed),
            ITERATIONS,
        );
    }
}
