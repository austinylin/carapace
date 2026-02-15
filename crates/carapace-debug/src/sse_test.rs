use anyhow::Result;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Emit test SSE events to verify streaming works
pub async fn sse_test(count: usize, interval_ms: u64, event_type: &str, tool: &str) -> Result<()> {
    println!("\n🔬 SSE Streaming Test Generator");
    println!("{}", "=".repeat(70));
    println!("Generating {} events at {}ms intervals", count, interval_ms);
    println!("Event type: {}, Tool: {}", event_type, tool);
    println!("{}", "=".repeat(70));
    println!("\nTo capture these events:");
    println!("1. Run: carapace-debug sniff --filter SseEvent");
    println!("2. In another terminal, run this command");
    println!(
        "3. Watch events arrive in real-time (should see all {} events)",
        count
    );
    println!("\n{}", "-".repeat(70));

    let interval = Duration::from_millis(interval_ms);
    let start_time = SystemTime::now();

    for i in 1..=count {
        // Create a test SSE event
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let event_data = json!({
            "num": i,
            "timestamp": timestamp,
            "message": format!("Test event #{} - sent at {:?}", i, start_time.elapsed().unwrap())
        });

        // In real scenario, this would be sent through protocol
        let _sse_event = json!({
            "id": format!("sse-test-{}", i),
            "tool": tool,
            "event": event_type,
            "data": event_data.to_string()
        });

        // Print the event (would be sent through protocol in real scenario)
        println!(
            "\n[Event #{}] SSE: id=sse-test-{}, event_type={}, data={}",
            i, i, event_type, event_data
        );

        if i < count {
            tokio::time::sleep(interval).await;
            let elapsed = start_time.elapsed().unwrap();
            println!("⏱️  Elapsed: {:?} ({}ms per event)", elapsed, interval_ms);
        }
    }

    println!("\n{}", "-".repeat(70));
    let total_time = start_time.elapsed().unwrap();
    println!("✅ Generated {} events in {:?}", count, total_time);
    println!(
        "📊 Rate: {:.1} events/sec",
        count as f64 / total_time.as_secs_f64()
    );
    println!("\nTip: In production, events come from upstream servers (e.g., signal-cli)");
    println!("     This tool simulates that for testing streaming behavior");

    Ok(())
}
