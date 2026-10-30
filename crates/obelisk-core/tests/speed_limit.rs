use std::time::Duration;

use futures_util::{StreamExt, stream};
use obelisk_core::transfer::{CHUNK_STALL, chunk_parts, set_speed_limit};
use tokio::time::Instant;

/// What a storefront's response delivers, in `parts` of `size` bytes, read
/// through the parts every storefront's content request reads.
async fn read(parts: usize, size: usize) -> Duration {
    let start = Instant::now();
    let response = stream::iter((0..parts).map(|_| Ok::<_, ()>(vec![0_u8; size])));
    let read: Vec<_> = chunk_parts(response).collect().await;
    assert!(read.iter().all(Result::is_ok));
    start.elapsed()
}

// One test: the limit is the process's, and tests of one binary share it.
#[tokio::test(start_paused = true)]
async fn the_speed_limit_holds_every_response_until_it_is_lifted() {
    assert_eq!(
        read(3, 12_500).await,
        Duration::ZERO,
        "no limit until one is set"
    );

    // 125 kB per second carries 12.5 kB in a tenth of a second.
    set_speed_limit(125_000);
    assert_eq!(read(3, 12_500).await, Duration::from_millis(300));

    // A part held longer than a stall allows is no stalled connection.
    let held = read(1, 4_000_000).await;
    assert_eq!(held, Duration::from_secs(32));
    assert!(held > CHUNK_STALL);

    set_speed_limit(0);
    assert_eq!(read(3, 12_500).await, Duration::ZERO);
}
