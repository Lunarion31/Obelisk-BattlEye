use std::time::Duration;

use futures_util::{StreamExt, stream};
use obelisk_core::transfer::{CHUNK_STALL, chunk_parts, set_speed_limit};
use tokio::time::Instant;

/// The lowest limit that holds.
const RATE: u64 = 625_000;

/// How long a storefront's response of one part of `size` bytes takes to
/// read through the parts every storefront's content request reads.
async fn read(size: usize) -> Duration {
    let start = Instant::now();
    let response = stream::once(async move { Ok::<_, ()>(vec![0_u8; size]) });
    let read: Vec<_> = chunk_parts(response).collect().await;
    assert!(read.iter().all(Result::is_ok));
    start.elapsed()
}

// One test: the limit is the process's, and tests of one binary share it.
#[tokio::test(start_paused = true)]
async fn the_speed_limit_holds_a_response_until_it_is_lifted() {
    // A part held for longer than a stall allows is no stalled connection.
    let held = CHUNK_STALL + Duration::from_secs(2);
    let size = (RATE * held.as_secs()) as usize;

    assert_eq!(
        read(size).await,
        Duration::ZERO,
        "no limit until one is set"
    );
    set_speed_limit(RATE);
    assert_eq!(read(size).await, held);
    set_speed_limit(0);
    assert_eq!(read(size).await, Duration::ZERO);
}
