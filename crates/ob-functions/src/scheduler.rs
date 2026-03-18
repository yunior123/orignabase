use crate::registry::FunctionRegistry;
use crate::runtime::WasmRuntime;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct CronScheduler {
    registry: Arc<FunctionRegistry>,
    runtime: Arc<WasmRuntime>,
}

impl CronScheduler {
    pub fn new(registry: Arc<FunctionRegistry>, runtime: Arc<WasmRuntime>) -> Self {
        Self { registry, runtime }
    }

    pub async fn run(self) {
        tracing::info!("Cron scheduler started");
        let mut last_run: HashMap<String, Instant> = HashMap::new();

        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;

            let cron_fns = self.registry.find_cron_triggers();
            for (name, schedule) in &cron_fns {
                let interval = parse_interval(schedule);
                let now = Instant::now();

                let should_run = match last_run.get(name) {
                    Some(last) => now.duration_since(*last) >= interval,
                    None => true,
                };

                if should_run {
                    last_run.insert(name.clone(), now);
                    let input = serde_json::json!({
                        "trigger": "cron",
                        "schedule": schedule,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    });

                    match self.registry.get_module(name) {
                        Ok(module) => {
                            match self
                                .runtime
                                .execute(&module, "handle", &input.to_string())
                                .await
                            {
                                Ok(result) => {
                                    tracing::info!(
                                        function = %name,
                                        "Cron function executed: {}",
                                        &result[..result.len().min(200)]
                                    );
                                }
                                Err(e) => {
                                    tracing::error!(function = %name, "Cron function failed: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                function = %name,
                                "Cron function module not found: {e}"
                            );
                        }
                    }
                }
            }
        }
    }
}

fn parse_interval(schedule: &str) -> Duration {
    match schedule.trim() {
        "@hourly" => Duration::from_secs(3600),
        "@daily" => Duration::from_secs(86400),
        s if s.starts_with("@every ") => {
            let rest = &s[7..];
            if let Some(mins) = rest.strip_suffix('m') {
                mins.parse::<u64>()
                    .map(|m| Duration::from_secs(m * 60))
                    .unwrap_or(Duration::from_secs(60))
            } else if let Some(hours) = rest.strip_suffix('h') {
                hours
                    .parse::<u64>()
                    .map(|h| Duration::from_secs(h * 3600))
                    .unwrap_or(Duration::from_secs(3600))
            } else {
                Duration::from_secs(60)
            }
        }
        "* * * * *" => Duration::from_secs(60),
        _ => Duration::from_secs(3600), // default: hourly
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interval_every_5m() {
        assert_eq!(parse_interval("@every 5m"), Duration::from_secs(300));
    }

    #[test]
    fn test_parse_interval_hourly() {
        assert_eq!(parse_interval("@hourly"), Duration::from_secs(3600));
    }

    #[test]
    fn test_parse_interval_daily() {
        assert_eq!(parse_interval("@daily"), Duration::from_secs(86400));
    }

    #[test]
    fn test_parse_interval_every_minute() {
        assert_eq!(parse_interval("* * * * *"), Duration::from_secs(60));
    }

    #[test]
    fn test_parse_interval_every_2h() {
        assert_eq!(parse_interval("@every 2h"), Duration::from_secs(7200));
    }

    #[test]
    fn test_parse_interval_unknown_defaults_hourly() {
        assert_eq!(parse_interval("0 */2 * * *"), Duration::from_secs(3600));
    }
}
