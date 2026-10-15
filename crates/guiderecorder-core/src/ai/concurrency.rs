//! Bounded-concurrency helpers for the per-step pass.

use std::future::Future;
use std::sync::Arc;

use tokio::task::JoinSet;

use super::{AiError, AiProvider, StepInput, StepOutput};

/// Run `f` on every item with at most `limit` futures in flight (a `limit`
/// of 0 is treated as 1). `on_done(index, output)` is called as each one
/// finishes, in completion order, so results can stream into the editor.
///
/// Must be called inside a Tokio runtime. A panic in a task is resumed on
/// the caller.
pub async fn for_each_bounded<T, R, F, Fut>(
    items: Vec<T>,
    limit: usize,
    f: F,
    mut on_done: impl FnMut(usize, R),
) where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = R> + Send + 'static,
{
    let limit = limit.max(1);
    let mut tasks = JoinSet::new();
    let mut pending = items.into_iter().enumerate();
    loop {
        while tasks.len() < limit {
            let Some((index, item)) = pending.next() else {
                break;
            };
            let fut = f(item);
            tasks.spawn(async move { (index, fut.await) });
        }
        let Some(joined) = tasks.join_next().await else {
            return;
        };
        match joined {
            Ok((index, output)) => on_done(index, output),
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            // Cancelled: the runtime is shutting down, so stop quietly.
            Err(_) => return,
        }
    }
}

/// Like [`for_each_bounded`], collecting outputs in input order.
pub async fn map_bounded<T, R, F, Fut>(items: Vec<T>, limit: usize, f: F) -> Vec<R>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = R> + Send + 'static,
{
    let mut out: Vec<Option<R>> = std::iter::repeat_with(|| None).take(items.len()).collect();
    for_each_bounded(items, limit, f, |index, output| out[index] = Some(output)).await;
    out.into_iter()
        .map(|output| output.expect("every task reports once"))
        .collect()
}

/// Default number of concurrent per-step calls.
pub const DEFAULT_STEP_CONCURRENCY: usize = 4;

/// Run the per-step pass over `inputs` with bounded concurrency, reporting
/// `(index, result)` as each step finishes.
pub async fn generate_steps(
    provider: Arc<dyn AiProvider>,
    inputs: Vec<StepInput>,
    limit: usize,
    on_done: impl FnMut(usize, Result<StepOutput, AiError>),
) {
    for_each_bounded(
        inputs,
        limit,
        |input| {
            let provider = Arc::clone(&provider);
            async move { provider.generate_step(&input).await }
        },
        on_done,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn limits_in_flight_and_keeps_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let items: Vec<u64> = (0..20).collect();
        let out = map_bounded(items, 3, |n| {
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            async move {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                // Later items finish sooner, so completion order differs.
                tokio::time::sleep(Duration::from_millis(20 - n)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                n * 10
            }
        })
        .await;
        assert_eq!(out, (0..20).map(|n| n * 10).collect::<Vec<_>>());
        assert_eq!(peak.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn zero_limit_and_empty_input() {
        assert_eq!(
            map_bounded(vec![1, 2], 0, |n| async move { n + 1 }).await,
            [2, 3]
        );
        let empty: Vec<u8> = map_bounded(Vec::<u8>::new(), 2, |n| async move { n }).await;
        assert!(empty.is_empty());
    }

    #[tokio::test]
    async fn reports_each_completion() {
        let mut seen = Vec::new();
        for_each_bounded(
            vec!["a", "b", "c"],
            2,
            |s| async move { s.len() },
            |i, r| seen.push((i, r)),
        )
        .await;
        seen.sort();
        assert_eq!(seen, [(0, 1), (1, 1), (2, 1)]);
    }

    #[tokio::test]
    #[should_panic(expected = "boom")]
    async fn propagates_panics() {
        map_bounded(vec![1], 1, |_: i32| async { panic!("boom") }).await;
    }
}
