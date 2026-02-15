use anyhow::Result;
use chrono::{Duration, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Query audit logs with filtering
#[allow(clippy::too_many_arguments)]
pub async fn audit(
    file: &Path,
    tool_filter: Option<String>,
    action_filter: Option<String>,
    result_filter: Option<String>,
    since_filter: Option<String>,
    follow: bool,
    format: &str,
    limit: usize,
) -> Result<()> {
    // Parse time filter
    let cutoff_time = since_filter.as_ref().and_then(|s| parse_time_filter(s));

    if follow {
        return Err(anyhow::anyhow!(
            "Follow mode is not implemented. Use: tail -f {}",
            file.display()
        ));
    }

    query_audit_log(
        file,
        tool_filter,
        action_filter,
        result_filter,
        cutoff_time,
        format,
        limit,
    )?;

    Ok(())
}

fn query_audit_log(
    file: &Path,
    tool_filter: Option<String>,
    action_filter: Option<String>,
    result_filter: Option<String>,
    cutoff_time: Option<chrono::DateTime<Utc>>,
    format: &str,
    limit: usize,
) -> Result<()> {
    if !file.exists() {
        println!("Audit log file not found: {}", file.display());
        return Ok(());
    }

    let f = File::open(file)?;
    let reader = BufReader::new(f);

    let mut entries = Vec::new();
    let mut count = 0;

    for line in reader.lines() {
        if count >= limit {
            break;
        }

        match line {
            Ok(line) => {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<Value>(&line) {
                    Ok(entry) => {
                        // Apply filters
                        if let Some(ref tool) = tool_filter {
                            if entry
                                .get("tool")
                                .and_then(|v| v.as_str())
                                .map(|t| t != tool)
                                .unwrap_or(true)
                            {
                                continue;
                            }
                        }

                        if let Some(ref action) = action_filter {
                            if entry
                                .get("action_type")
                                .and_then(|v| v.as_str())
                                .map(|a| a != action)
                                .unwrap_or(true)
                            {
                                continue;
                            }
                        }

                        if let Some(ref result) = result_filter {
                            if entry
                                .get("policy_result")
                                .and_then(|v| v.as_str())
                                .map(|r| r != result)
                                .unwrap_or(true)
                            {
                                continue;
                            }
                        }

                        if let Some(cutoff) = cutoff_time {
                            if let Some(timestamp_str) =
                                entry.get("timestamp").and_then(|v| v.as_str())
                            {
                                if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(timestamp_str)
                                {
                                    if ts.with_timezone(&Utc) < cutoff {
                                        continue;
                                    }
                                }
                            }
                        }

                        entries.push(entry);
                        count += 1;
                    }
                    Err(_) => {
                        // Skip malformed JSON lines
                        continue;
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading log file: {}", e);
            }
        }
    }

    // Reverse to show newest first
    entries.reverse();

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        // Text format with table
        print_audit_table(&entries);
    }

    Ok(())
}

fn print_audit_table(entries: &[Value]) {
    println!("=== Audit Log Entries (Most Recent First) ===");
    println!(
        "{:<20} {:<12} {:<8} {:<15} {:<40}",
        "Timestamp", "Tool", "Action", "Result", "Details"
    );
    println!("{}", "-".repeat(100));

    for entry in entries {
        let timestamp = entry
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.split('T').next().unwrap_or(s))
            .unwrap_or("unknown");

        let tool = entry
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let action = entry
            .get("action_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        let result = entry
            .get("policy_result")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Build details from different fields
        let details = if let Some(argv) = entry.get("argv").and_then(|v| v.as_array()) {
            let args: Vec<String> = argv
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            args.join(" ").chars().take(40).collect::<String>()
        } else if let Some(method) = entry.get("method").and_then(|v| v.as_str()) {
            format!(
                "{} {}",
                method,
                entry.get("path").and_then(|v| v.as_str()).unwrap_or("")
            )
            .chars()
            .take(40)
            .collect()
        } else if let Some(msg) = entry.get("error_message").and_then(|v| v.as_str()) {
            msg.chars().take(40).collect::<String>()
        } else {
            "-".to_string()
        };

        println!(
            "{:<20} {:<12} {:<8} {:<15} {:<40}",
            timestamp, tool, action, result, details
        );
    }

    println!("\nTotal: {} entries", entries.len());
}

/// Parse time filter like "5m", "1h", "24h"
fn parse_time_filter(filter: &str) -> Option<chrono::DateTime<Utc>> {
    let now = Utc::now();

    match filter {
        s if s.ends_with('m') => {
            let minutes: i64 = s.trim_end_matches('m').parse().ok()?;
            Some(now - Duration::minutes(minutes))
        }
        s if s.ends_with('h') => {
            let hours: i64 = s.trim_end_matches('h').parse().ok()?;
            Some(now - Duration::hours(hours))
        }
        s if s.ends_with('d') => {
            let days: i64 = s.trim_end_matches('d').parse().ok()?;
            Some(now - Duration::days(days))
        }
        _ => None,
    }
}
