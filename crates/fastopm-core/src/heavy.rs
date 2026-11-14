//! A small fixed pool of threads for allocation-heavy work: parsing, merging, writing,
//! rendering and compressing manifests that can be tens of megabytes.
//!
//! Tokio's blocking pool grows to hundreds of threads under load, and every allocator
//! (glibc malloc arenas, mimalloc thread heaps) keeps per-thread memory: a tree parsed on one of
//! those threads and freed later stays in that thread's heap until the thread allocates again
//! or exits. Running the heavy work on the same few threads lets freed memory be reused right
//! away, so RSS follows the live heap. It also caps how many large documents are processed at
//! once (one per thread; extra jobs queue).
//!
//! The pool has as many threads as [`std::thread::available_parallelism`] (which honours the
//! cgroup CPU quota), between 2 and 32, started on first use.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

type Job = Box<dyn FnOnce() + Send>;

struct Pool {
    jobs: Sender<Job>,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(2, 32);
        let (tx, rx) = channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads {
            let rx = Arc::clone(&rx);
            std::thread::Builder::new()
                .name(format!("fastopm-heavy-{i}"))
                .spawn(move || worker(&rx))
                .expect("cannot start the fastopm-heavy threads");
        }
        Pool { jobs: tx }
    })
}

fn worker(rx: &Mutex<Receiver<Job>>) {
    loop {
        let job = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
        match job {
            Ok(job) => job(),
            Err(_) => return,
        }
    }
}

/// Run `work` on the pool and wait for it. A panic in `work` resumes on the caller.
pub async fn run<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let job: Job = Box::new(move || {
        let _ = tx.send(catch_unwind(AssertUnwindSafe(work)));
    });
    let sent = pool().jobs.send(job);
    if sent.is_err() {
        panic!("fastopm-heavy pool stopped");
    }
    match rx.await {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("fastopm-heavy job dropped"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_jobs_on_the_pool_threads() {
        let name = run(|| std::thread::current().name().map(str::to_owned)).await;
        assert!(name.unwrap().starts_with("fastopm-heavy-"));
        let sums = futures_util_join((0..64u64).map(|i| run(move || i * 2)).collect()).await;
        assert_eq!(sums.iter().sum::<u64>(), 64 * 63);
    }

    #[tokio::test]
    async fn panics_resume_on_the_caller() {
        let caught = tokio::spawn(run(|| panic!("boom"))).await;
        assert!(caught.unwrap_err().is_panic());
        // the pool still works
        assert_eq!(run(|| 7).await, 7);
    }

    async fn futures_util_join<F: std::future::Future>(futures: Vec<F>) -> Vec<F::Output> {
        let mut out = Vec::with_capacity(futures.len());
        for f in futures {
            out.push(f.await);
        }
        out
    }
}
