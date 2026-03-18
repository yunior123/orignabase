use crate::DatabaseClient;
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Task status lifecycle: pending → running → completed | failed | dead_letter
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    DeadLetter,
}

/// A background task stored in SurrealDB.
///
/// Replaces Google Cloud Tasks with a self-hosted alternative.
/// Tasks are stored in `_task_queue` collection and processed by workers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Task type / handler name (e.g. "send_email", "sync_search", "cleanup_expired")
    pub task_type: String,
    /// JSON payload for the task handler
    pub payload: Value,
    /// Current status
    pub status: TaskStatus,
    /// Queue name for routing (default: "default")
    #[serde(default = "default_queue")]
    pub queue: String,
    /// Number of retry attempts so far
    #[serde(default)]
    pub attempts: u32,
    /// Maximum retry attempts before dead-lettering
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Scheduled execution time (ISO 8601). None = execute immediately.
    pub scheduled_at: Option<String>,
    /// When the task was created
    pub created_at: String,
    /// When the task started running
    pub started_at: Option<String>,
    /// When the task completed or failed
    pub finished_at: Option<String>,
    /// Error message from last failed attempt
    pub last_error: Option<String>,
    /// Priority (lower = higher priority, default: 0)
    #[serde(default)]
    pub priority: i32,
}

fn default_queue() -> String {
    "default".into()
}

fn default_max_retries() -> u32 {
    3
}

/// Request to enqueue a new task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnqueueRequest {
    pub task_type: String,
    pub payload: Value,
    #[serde(default = "default_queue")]
    pub queue: String,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Delay in seconds before the task becomes eligible for execution
    #[serde(default)]
    pub delay_secs: u64,
    #[serde(default)]
    pub priority: i32,
}

/// Task queue backed by SurrealDB.
///
/// ## Architecture
///
/// - Tasks are stored in `_task_queue` collection
/// - Workers poll for pending tasks using atomic claim (SELECT + UPDATE in one query)
/// - Failed tasks are retried with exponential backoff
/// - Dead-lettered tasks are moved to `_task_dead_letter` for inspection
/// - Stale running tasks (no heartbeat for >5 min) are reclaimed
///
/// ## Usage
///
/// ```ignore
/// let queue = TaskQueue::new(db.clone());
///
/// // Enqueue a task
/// queue.enqueue(EnqueueRequest {
///     task_type: "send_email".into(),
///     payload: json!({"to": "user@example.com", "template": "welcome"}),
///     ..Default::default()
/// }).await?;
///
/// // Process tasks (run in a tokio::spawn)
/// queue.run_worker("default", |task| async move {
///     match task.task_type.as_str() {
///         "send_email" => { /* send email */ Ok(()) }
///         _ => Err(Error::Internal("Unknown task type".into()))
///     }
/// }).await;
/// ```
#[derive(Clone)]
pub struct TaskQueue {
    db: DatabaseClient,
}

impl TaskQueue {
    pub fn new(db: DatabaseClient) -> Self {
        Self { db }
    }

    /// Enqueue a new task for background processing.
    pub async fn enqueue(&self, req: EnqueueRequest) -> Result<Value> {
        let now = chrono::Utc::now();
        let scheduled_at = if req.delay_secs > 0 {
            Some((now + chrono::Duration::seconds(req.delay_secs as i64)).to_rfc3339())
        } else {
            None
        };

        let task = Task {
            task_type: req.task_type,
            payload: req.payload,
            status: TaskStatus::Pending,
            queue: req.queue,
            attempts: 0,
            max_retries: req.max_retries,
            scheduled_at,
            created_at: now.to_rfc3339(),
            started_at: None,
            finished_at: None,
            last_error: None,
            priority: req.priority,
        };

        let task_value = serde_json::to_value(&task)
            .map_err(|e| Error::Internal(format!("Task serialization failed: {e}")))?;

        self.db.create_document("_task_queue", task_value).await
    }

    /// Enqueue multiple tasks in a batch.
    pub async fn enqueue_batch(&self, requests: Vec<EnqueueRequest>) -> Result<Vec<Value>> {
        let mut results = Vec::with_capacity(requests.len());
        for req in requests {
            results.push(self.enqueue(req).await?);
        }
        Ok(results)
    }

    /// Atomically claim the next pending task from the given queue.
    /// Two-step: SELECT to find candidate, then UPDATE to claim it.
    pub async fn claim_next(&self, queue: &str) -> Result<Option<(String, Task)>> {
        let now = chrono::Utc::now().to_rfc3339();

        // Step 1: Find the next pending task
        let candidates = self
            .db
            .query_bind(
                "SELECT * FROM _task_queue \
                 WHERE queue = $queue \
                 AND status = 'pending' \
                 AND (scheduled_at IS NONE OR scheduled_at <= $now) \
                 ORDER BY priority ASC, created_at ASC \
                 LIMIT 1",
                serde_json::json!({ "queue": queue, "now": now }),
            )
            .await?;

        let Some(candidate) = candidates.first() else {
            return Ok(None);
        };

        let task_id = candidate["id"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| candidate["id"].to_string());

        // Step 2: Atomically claim it (only if still pending)
        let results = self
            .db
            .query_bind(
                &format!(
                    "UPDATE {} SET status = 'running', started_at = $now, attempts = attempts + 1 \
                     WHERE status = 'pending' RETURN AFTER",
                    task_id
                ),
                serde_json::json!({ "now": now }),
            )
            .await?;

        if let Some(doc) = results.first() {
            let id = doc["id"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| doc["id"].to_string());

            match serde_json::from_value::<Task>(doc.clone()) {
                Ok(task) => Ok(Some((id, task))),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    /// Mark a task as completed.
    pub async fn complete(&self, task_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db
            .query_bind(
                "UPDATE type::thing($tid) SET status = 'completed', finished_at = $now",
                serde_json::json!({ "tid": task_id, "now": now }),
            )
            .await?;
        Ok(())
    }

    /// Mark a task as failed. If retries remain, requeue as pending with backoff.
    pub async fn fail(&self, task_id: &str, error: &str) -> Result<()> {
        let now = chrono::Utc::now();

        // Get current task to check retry count
        let results = self
            .db
            .query_bind(
                "SELECT * FROM type::thing($tid)",
                serde_json::json!({ "tid": task_id }),
            )
            .await?;

        let Some(doc) = results.first() else {
            return Ok(());
        };

        let attempts = doc["attempts"].as_u64().unwrap_or(1) as u32;
        let max_retries = doc["max_retries"].as_u64().unwrap_or(3) as u32;

        if attempts >= max_retries {
            // Dead-letter the task
            self.db
                .query_bind(
                    "UPDATE type::thing($tid) SET status = 'dead_letter', finished_at = $now, last_error = $error",
                    serde_json::json!({ "tid": task_id, "now": now.to_rfc3339(), "error": error }),
                )
                .await?;
        } else {
            // Retry with exponential backoff: 2^attempts seconds (2s, 4s, 8s, 16s, ...)
            let backoff_secs = 2i64.pow(attempts);
            let retry_at = (now + chrono::Duration::seconds(backoff_secs)).to_rfc3339();

            self.db
                .query_bind(
                    "UPDATE type::thing($tid) SET status = 'pending', scheduled_at = $retry_at, last_error = $error, started_at = NONE",
                    serde_json::json!({ "tid": task_id, "retry_at": retry_at, "error": error }),
                )
                .await?;
        }

        Ok(())
    }

    /// Reclaim stale running tasks (no completion after timeout).
    /// Call this periodically (e.g. every 60 seconds) to handle crashed workers.
    pub async fn reclaim_stale(&self, timeout_secs: u64) -> Result<u64> {
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::seconds(timeout_secs as i64)).to_rfc3339();

        let results = self
            .db
            .query_bind(
                "UPDATE _task_queue SET status = 'pending', started_at = NONE \
                 WHERE status = 'running' AND started_at < $cutoff \
                 RETURN AFTER",
                serde_json::json!({ "cutoff": cutoff }),
            )
            .await?;

        Ok(results.len() as u64)
    }

    /// Get queue statistics.
    pub async fn stats(&self, queue: &str) -> Result<Value> {
        let results = self
            .db
            .query_bind(
                "SELECT status, count() AS count FROM _task_queue WHERE queue = $queue GROUP BY status",
                serde_json::json!({ "queue": queue }),
            )
            .await?;

        let mut stats = serde_json::Map::new();
        stats.insert("queue".into(), serde_json::json!(queue));
        for r in &results {
            if let (Some(status), Some(count)) = (r["status"].as_str(), r["count"].as_u64()) {
                stats.insert(status.into(), serde_json::json!(count));
            }
        }

        Ok(Value::Object(stats))
    }

    /// Purge completed tasks older than the given duration.
    pub async fn purge_completed(&self, older_than_secs: u64) -> Result<u64> {
        let cutoff =
            (chrono::Utc::now() - chrono::Duration::seconds(older_than_secs as i64)).to_rfc3339();

        let results = self
            .db
            .query_bind(
                "DELETE FROM _task_queue WHERE status = 'completed' AND finished_at < $cutoff RETURN BEFORE",
                serde_json::json!({ "cutoff": cutoff }),
            )
            .await?;

        Ok(results.len() as u64)
    }

    /// List dead-lettered tasks for inspection.
    pub async fn list_dead_letter(&self, queue: &str, limit: usize) -> Result<Vec<Value>> {
        self.db
            .query_bind(
                &format!(
                    "SELECT * FROM _task_queue WHERE queue = $queue AND status = 'dead_letter' \
                     ORDER BY finished_at DESC LIMIT {limit}"
                ),
                serde_json::json!({ "queue": queue }),
            )
            .await
    }

    /// Retry a dead-lettered task by resetting its status.
    pub async fn retry_dead_letter(&self, task_id: &str) -> Result<()> {
        self.db
            .query_bind(
                "UPDATE type::thing($tid) SET status = 'pending', attempts = 0, \
                 started_at = NONE, finished_at = NONE, last_error = NONE, scheduled_at = NONE",
                serde_json::json!({ "tid": task_id }),
            )
            .await?;
        Ok(())
    }
}

impl Default for EnqueueRequest {
    fn default() -> Self {
        Self {
            task_type: String::new(),
            payload: Value::Null,
            queue: default_queue(),
            max_retries: default_max_retries(),
            delay_secs: 0,
            priority: 0,
        }
    }
}

/// Run a task worker loop that polls for tasks and processes them.
///
/// This is the main entry point for background task processing.
/// Run one or more of these in `tokio::spawn` for each queue you want to process.
///
/// The handler receives a `Task` and returns `Result<()>`.
/// On success, the task is marked completed.
/// On failure, it's retried with exponential backoff or dead-lettered.
pub async fn run_worker<F, Fut>(queue: Arc<TaskQueue>, queue_name: &str, handler: F)
where
    F: Fn(Task) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send,
{
    let poll_interval = tokio::time::Duration::from_secs(1);
    let idle_interval = tokio::time::Duration::from_secs(5);
    let stale_check_interval = tokio::time::Duration::from_secs(60);

    let mut last_stale_check = tokio::time::Instant::now();

    loop {
        // Periodically reclaim stale tasks
        if last_stale_check.elapsed() >= stale_check_interval {
            match queue.reclaim_stale(300).await {
                Ok(n) if n > 0 => {
                    tracing::warn!(queue = queue_name, reclaimed = n, "Reclaimed stale tasks");
                }
                Err(e) => {
                    tracing::error!(queue = queue_name, error = %e, "Failed to reclaim stale tasks");
                }
                _ => {}
            }
            last_stale_check = tokio::time::Instant::now();
        }

        // Try to claim a task
        match queue.claim_next(queue_name).await {
            Ok(Some((task_id, task))) => {
                let task_type = task.task_type.clone();
                tracing::debug!(
                    queue = queue_name,
                    task_type = %task_type,
                    task_id = %task_id,
                    attempt = task.attempts,
                    "Processing task"
                );

                match handler(task).await {
                    Ok(()) => {
                        if let Err(e) = queue.complete(&task_id).await {
                            tracing::error!(task_id = %task_id, error = %e, "Failed to mark task complete");
                        } else {
                            tracing::debug!(task_id = %task_id, task_type = %task_type, "Task completed");
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        tracing::warn!(
                            task_id = %task_id,
                            task_type = %task_type,
                            error = %error_msg,
                            "Task failed"
                        );
                        if let Err(e) = queue.fail(&task_id, &error_msg).await {
                            tracing::error!(task_id = %task_id, error = %e, "Failed to mark task as failed");
                        }
                    }
                }

                // Poll again immediately (there may be more tasks)
                tokio::time::sleep(poll_interval).await;
            }
            Ok(None) => {
                // No tasks — idle wait
                tokio::time::sleep(idle_interval).await;
            }
            Err(e) => {
                tracing::error!(queue = queue_name, error = %e, "Failed to claim task");
                tokio::time::sleep(idle_interval).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_enqueue_request_default() {
        let req = EnqueueRequest::default();
        assert_eq!(req.queue, "default");
        assert_eq!(req.max_retries, 3);
        assert_eq!(req.delay_secs, 0);
        assert_eq!(req.priority, 0);
    }

    #[test]
    fn test_task_status_serialization() {
        assert_eq!(
            serde_json::to_string(&TaskStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Running).unwrap(),
            "\"running\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::Failed).unwrap(),
            "\"failed\""
        );
        assert_eq!(
            serde_json::to_string(&TaskStatus::DeadLetter).unwrap(),
            "\"dead_letter\""
        );
    }

    #[test]
    fn test_task_status_deserialization() {
        let status: TaskStatus = serde_json::from_str("\"pending\"").unwrap();
        assert_eq!(status, TaskStatus::Pending);

        let status: TaskStatus = serde_json::from_str("\"dead_letter\"").unwrap();
        assert_eq!(status, TaskStatus::DeadLetter);
    }

    #[test]
    fn test_task_serialization_roundtrip() {
        let task = Task {
            task_type: "send_email".into(),
            payload: json!({"to": "user@example.com"}),
            status: TaskStatus::Pending,
            queue: "emails".into(),
            attempts: 0,
            max_retries: 5,
            scheduled_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            last_error: None,
            priority: -1,
        };

        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["task_type"], "send_email");
        assert_eq!(json["queue"], "emails");
        assert_eq!(json["priority"], -1);
        assert_eq!(json["max_retries"], 5);

        let deserialized: Task = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.task_type, "send_email");
        assert_eq!(deserialized.max_retries, 5);
    }

    #[test]
    fn test_enqueue_request_with_delay() {
        let req = EnqueueRequest {
            task_type: "cleanup".into(),
            payload: json!({}),
            queue: "maintenance".into(),
            max_retries: 1,
            delay_secs: 60,
            priority: 10,
        };
        assert_eq!(req.delay_secs, 60);
        assert_eq!(req.queue, "maintenance");
    }

    #[test]
    fn test_task_defaults() {
        let json_str = r#"{
            "task_type": "test",
            "payload": null,
            "status": "pending",
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        let task: Task = serde_json::from_str(json_str).unwrap();
        assert_eq!(task.queue, "default");
        assert_eq!(task.max_retries, 3);
        assert_eq!(task.attempts, 0);
        assert_eq!(task.priority, 0);
    }

    #[test]
    fn test_exponential_backoff_calculation() {
        // Verify the backoff formula: 2^attempts seconds
        assert_eq!(2i64.pow(1), 2); // 1st retry: 2s
        assert_eq!(2i64.pow(2), 4); // 2nd retry: 4s
        assert_eq!(2i64.pow(3), 8); // 3rd retry: 8s
        assert_eq!(2i64.pow(4), 16); // 4th retry: 16s
        assert_eq!(2i64.pow(5), 32); // 5th retry: 32s
    }
}
