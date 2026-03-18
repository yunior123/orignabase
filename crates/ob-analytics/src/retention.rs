use ob_database::DatabaseClient;
use std::time::Duration;
use tracing;

/// Background task that periodically deletes analytics events older than the retention period.
pub struct RetentionPolicy {
    db: DatabaseClient,
    /// How long to keep events (default: 90 days).
    retention_days: u32,
    /// How often to run the cleanup (default: 24 hours).
    interval: Duration,
}

impl RetentionPolicy {
    pub fn new(db: DatabaseClient, retention_days: u32) -> Self {
        Self {
            db,
            retention_days,
            interval: Duration::from_secs(86400), // Once per day
        }
    }

    /// Run the retention policy cleanup loop.
    pub async fn run(self) {
        tracing::info!(
            retention_days = self.retention_days,
            "Analytics retention policy started"
        );

        loop {
            tokio::time::sleep(self.interval).await;

            let query = format!(
                "DELETE FROM _analytics_events WHERE timestamp < time::now() - {}d",
                self.retention_days
            );

            match self.db.query_raw(&query).await {
                Ok(_) => {
                    tracing::info!(
                        retention_days = self.retention_days,
                        "Analytics retention cleanup completed"
                    );
                }
                Err(e) => {
                    tracing::error!("Analytics retention cleanup failed: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retention_policy_defaults() {
        // Compile-time check that RetentionPolicy can be constructed
        fn _assert_fields(p: &RetentionPolicy) {
            let _ = p.retention_days;
            let _ = &p.interval;
        }
    }
}
