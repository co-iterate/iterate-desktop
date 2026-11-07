//! Claude Code subscription quota.
//!
//! The numbers come from the `rate_limits` field Claude Code hands to its
//! status line; the user's status line script copies the percentages and reset
//! times into `~/.cunzhi/claude-rate-limits.json`. iterate never reads Claude
//! credentials. The snapshot is only refreshed while Claude Code is active, so
//! the provider summary always states when it was captured.

use super::quota::{QuotaMetric, UsageProvider};
use chrono::{DateTime, Local};
use serde::Deserialize;
use std::path::PathBuf;

const CLAUDE_RATE_LIMIT_SNAPSHOT_FILE: &str = "claude-rate-limits.json";

#[derive(Debug, Deserialize)]
struct ClaudeRateLimitSnapshot {
    captured_at: i64,
    five_hour: Option<ClaudeRateLimitWindow>,
    seven_day: Option<ClaudeRateLimitWindow>,
}

#[derive(Debug, Deserialize)]
struct ClaudeRateLimitWindow {
    used_percentage: f64,
    resets_at: i64,
}

fn claude_rate_limit_snapshot_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".cunzhi").join(CLAUDE_RATE_LIMIT_SNAPSHOT_FILE))
}

pub(super) async fn get_claude_code_usage_provider() -> Result<UsageProvider, String> {
    let path = claude_rate_limit_snapshot_path()
        .ok_or_else(|| "无法解析用户 home 目录".to_string())?;
    let text = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| "还没有 Claude Code 额度快照，需要 statusline 脚本写入".to_string())?;
    claude_provider_from_snapshot(&text, Local::now())
}

fn claude_provider_from_snapshot(text: &str, now: DateTime<Local>) -> Result<UsageProvider, String> {
    let snapshot: ClaudeRateLimitSnapshot = serde_json::from_str(text)
        .map_err(|error| format!("Claude Code 额度快照格式错误：{}", error))?;
    let captured_at = DateTime::from_timestamp(snapshot.captured_at, 0)
        .map(|date| date.with_timezone(&Local))
        .ok_or_else(|| "Claude Code 额度快照时间无效".to_string())?;

    let metrics: Vec<QuotaMetric> = [
        ("Session", snapshot.five_hour.as_ref()),
        ("Weekly", snapshot.seven_day.as_ref()),
    ]
    .into_iter()
    .filter_map(|(label, window)| window.map(|window| claude_window_metric(label, window, now)))
    .collect();
    if metrics.is_empty() {
        return Err("Claude Code 额度快照里没有额度窗口".to_string());
    }

    Ok(UsageProvider {
        id: "claude".to_string(),
        name: "Claude Code".to_string(),
        account_label: None,
        color: "#d97757".to_string(),
        icon_url: Some("./icons/ai-providers/claude.svg".to_string()),
        summary: format!("截至 {}", captured_at.format("%-m月%-d日 %-H:%M")),
        updated_at: Some(captured_at.format("%H:%M").to_string()),
        metrics,
    })
}

fn claude_window_metric(label: &str, window: &ClaudeRateLimitWindow, now: DateTime<Local>) -> QuotaMetric {
    let reset_at = DateTime::from_timestamp(window.resets_at, 0).map(|date| date.with_timezone(&Local));
    if reset_at.is_some_and(|reset_at| reset_at <= now) {
        // The window rolled over after the snapshot; its new usage is unknown
        // until Claude Code reports again, so do not show the stale figure.
        return QuotaMetric {
            label: label.to_string(),
            remaining: 100,
            reset_label: Some("已重置，待 Claude Code 更新".to_string()),
            reset_at_ms: None,
        };
    }
    QuotaMetric {
        label: label.to_string(),
        remaining: (100.0 - window.used_percentage).clamp(0.0, 100.0).round() as u8,
        reset_label: reset_at.map(|date| format!("{} 重置", date.format("%Y年%-m月%-d日 %-H:%M"))),
        reset_at_ms: reset_at.map(|date| date.timestamp_millis()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(seconds: i64) -> DateTime<Local> {
        DateTime::from_timestamp(seconds, 0).unwrap().with_timezone(&Local)
    }

    #[test]
    fn snapshot_maps_used_percentage_to_remaining() {
        let provider = claude_provider_from_snapshot(
            r#"{"schema":1,"captured_at":1000,"five_hour":{"used_percentage":8,"resets_at":5000},"seven_day":{"used_percentage":63.4,"resets_at":9000}}"#,
            local(2000),
        )
        .unwrap();
        assert_eq!(provider.id, "claude");
        let remaining: Vec<(&str, u8)> = provider
            .metrics
            .iter()
            .map(|metric| (metric.label.as_str(), metric.remaining))
            .collect();
        assert_eq!(remaining, vec![("Session", 92), ("Weekly", 37)]);
        assert_eq!(provider.metrics[0].reset_at_ms, Some(5_000_000));
    }

    #[test]
    fn window_past_its_reset_is_not_shown_as_current_usage() {
        let provider = claude_provider_from_snapshot(
            r#"{"captured_at":1000,"five_hour":{"used_percentage":95,"resets_at":1500},"seven_day":{"used_percentage":40,"resets_at":9000}}"#,
            local(2000),
        )
        .unwrap();
        let session = &provider.metrics[0];
        assert_eq!(session.remaining, 100);
        assert_eq!(session.reset_label.as_deref(), Some("已重置，待 Claude Code 更新"));
        assert_eq!(provider.metrics[1].remaining, 60);
    }

    #[test]
    fn missing_windows_or_bad_json_are_errors() {
        assert!(claude_provider_from_snapshot(r#"{"captured_at":1000}"#, local(2000)).is_err());
        assert!(claude_provider_from_snapshot("not json", local(2000)).is_err());
    }
}
