//! Running independent jobs at once, with results in job order (so output
//! never depends on which thread finished first).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Threads worth using for pure computation on this machine.
pub fn threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

/// `f(lane, job)` for every job, on up to `lanes` threads. A lane is a number
/// below `lanes` that no two running jobs share (a private worktree, say).
/// One lane, or one job, runs on the calling thread.
pub fn map<J: Sync, T: Send>(lanes: usize, jobs: &[J], f: impl Fn(usize, &J) -> T + Sync) -> Vec<T> {
    let lanes = lanes.clamp(1, jobs.len().max(1));
    if lanes == 1 {
        return jobs.iter().map(|j| f(0, j)).collect();
    }
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<T>>> = Mutex::new(jobs.iter().map(|_| None).collect());
    std::thread::scope(|s| {
        for lane in 0..lanes {
            let (next, out, f) = (&next, &out, &f);
            s.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let r = f(lane, job);
                out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
            });
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter().map(|r| r.expect("a job panicked")).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn results_keep_job_order_and_lanes_are_exclusive() {
        let jobs: Vec<u64> = (0..200).collect();
        let busy: Vec<std::sync::atomic::AtomicBool> = (0..4).map(|_| false.into()).collect();
        let out = super::map(4, &jobs, |lane, j| {
            assert!(!busy[lane].swap(true, std::sync::atomic::Ordering::SeqCst), "two jobs on one lane");
            std::thread::yield_now();
            busy[lane].store(false, std::sync::atomic::Ordering::SeqCst);
            j * j
        });
        assert_eq!(out, jobs.iter().map(|j| j * j).collect::<Vec<_>>());
        assert_eq!(super::map(1, &jobs, |lane, j| j + lane as u64), jobs);
        assert!(super::map(8, &[] as &[u64], |_, j| *j).is_empty());
    }
}
