//! Work shared out among threads, with the standard library only. (The crate's own: bc7enc_rdo
//! uses OpenMP.)

use std::sync::Mutex;

/// How many threads to use for a setting where 0 means all the machine has.
pub(crate) fn worker_count(threads: u32) -> usize {
    if threads != 0 {
        threads as usize
    } else {
        std::thread::available_parallelism().map_or(1, |count| count.get())
    }
}

/// Calls `work` for every item, on up to `threads` threads at once. The items are handed out
/// one by one as threads come free, so their order of completion is not fixed; what each call
/// does must not depend on the others.
pub(crate) fn for_each<T, F>(items: Vec<T>, threads: usize, work: F)
where
    T: Send,
    F: Fn(T) + Sync,
{
    let threads = threads.min(items.len());
    if threads <= 1 {
        items.into_iter().for_each(work);
        return;
    }
    let queue = Mutex::new(items.into_iter());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    // (The lock is let go before the work is done.)
                    let next = queue
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .next();
                    match next {
                        Some(item) => work(item),
                        None => break,
                    }
                }
            });
        }
    });
}

/// Calls `work` for runs of blocks and the runs of pixels that go with them, on up to
/// `threads` threads (0 for all the machine has). A few blocks are not worth a thread: under
/// 64 they are done where they are.
pub(crate) fn for_each_run<B, P, F>(blocks: &mut [B], pixels: &[P], threads: u32, work: F)
where
    B: Send,
    P: Sync,
    F: Fn(&mut [B], &[P]) + Sync,
{
    let threads = worker_count(threads);
    if threads <= 1 || blocks.len() < 64 {
        work(blocks, pixels);
        return;
    }
    // Several runs for each thread, so that a thread with easy blocks takes more of them.
    let run = (blocks.len() / (threads * 4)).max(16);
    let runs: Vec<(&mut [B], &[P])> = blocks.chunks_mut(run).zip(pixels.chunks(run)).collect();
    for_each(runs, threads, |(blocks, pixels)| work(blocks, pixels));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn every_item_is_done_once_whatever_the_thread_count() {
        for threads in [0usize, 1, 2, 3, 16] {
            let mut data = vec![0u32; 1000];
            let total = AtomicU32::new(0);
            let items: Vec<(usize, &mut [u32])> = data.chunks_mut(7).enumerate().collect();
            for_each(items, threads, |(index, chunk)| {
                for value in chunk.iter_mut() {
                    *value += 1 + index as u32;
                }
                total.fetch_add(chunk.len() as u32, Ordering::Relaxed);
            });
            assert_eq!(total.load(Ordering::Relaxed), 1000);
            assert!(
                data.iter()
                    .enumerate()
                    .all(|(i, &value)| value == 1 + (i / 7) as u32)
            );
        }
        assert!(worker_count(0) >= 1);
        assert_eq!(worker_count(5), 5);
    }
}
