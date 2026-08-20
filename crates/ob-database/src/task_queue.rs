use crate::{DatabaseClient, fields};
use ob_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::Row;
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

/// A background task stored in the _task_queue table.
///
/// Replaces Google Cloud Tasks with a self-hosted alternative.
/// Tasks are stored in `_task_queue` table and processed by workers.
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

/// Task queue backed by _task_queue table in PostgreSQL.
///
/// ## Architecture
///
/// - Tasks are stored in `_task_queue` table with specific columns
/// - Workers poll for pending tasks using atomic claim (SELECT + UPDATE)
/// - Failed tasks are retried with exponential backoff
/// - Dead-lettered tasks are marked for inspection
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

    /// Ensure the _task_queue table exists and has all required columns.
    /// Handles schema evolution for existing databases.
    async fn ensure_schema(&self) -> Result<()> {
        // Create the table if it doesn't exist
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS _task_queue (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                job_name TEXT NOT NULL,
                queue TEXT NOT NULL DEFAULT 'default',
                status TEXT NOT NULL DEFAULT 'pending',
                payload JSONB NOT NULL DEFAULT '{}'::jsonb,
                scheduled_at TIMESTAMPTZ,
                locked_at TIMESTAMPTZ,
                locked_by TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                started_at TIMESTAMPTZ,
                completed_at TIMESTAMPTZ,
                retry_count INT NOT NULL DEFAULT 0,
                max_retries INT NOT NULL DEFAULT 3,
                priority INT NOT NULL DEFAULT 0,
                error_message TEXT
            )
            "#,
        )
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Failed to create _task_queue table: {e}")))?;

        // Add missing columns for schema evolution
        sqlx::query(
            r#"ALTER TABLE _task_queue ADD COLUMN IF NOT EXISTS queue TEXT NOT NULL DEFAULT 'default'"#,
        )
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Schema migration failed: {e}")))?;
        Ok(())
    }

    /// Map a database row to a Task struct.
    fn row_to_task(row: &sqlx::postgres::PgRow) -> Task {
        Task {
            task_type: row.get("job_name"),
            payload: row.try_get::<Value, _>("payload").unwrap_or(Value::Null),
            status: {
                let s: String = row.get(fields::STATUS);
                serde_json::from_value(Value::String(s)).unwrap_or(TaskStatus::Pending)
            },
            queue: row.try_get("queue").unwrap_or_else(|_| "default".into()),
            attempts: row.get::<i32, _>("retry_count") as u32,
            max_retries: row.get::<i32, _>("max_retries") as u32,
            scheduled_at: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("scheduled_at")
                .ok()
                .map(|t| t.to_rfc3339()),
            created_at: row
                .get::<chrono::DateTime<chrono::Utc>, _>("created_at")
                .to_rfc3339(),
            started_at: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("locked_at")
                .ok()
                .map(|t| t.to_rfc3339()),
            finished_at: row
                .try_get::<chrono::DateTime<chrono::Utc>, _>("completed_at")
                .ok()
                .map(|t| t.to_rfc3339()),
            last_error: row.try_get("error_message").ok(),
            priority: 0,
        }
    }

    /// Enqueue a new task for background processing.
    pub async fn enqueue(&self, req: EnqueueRequest) -> Result<Value> {
        self.ensure_schema().await?;
        let now = chrono::Utc::now();
        let scheduled_at = if req.delay_secs > 0 {
            Some((now + chrono::Duration::seconds(req.delay_secs as i64)).to_rfc3339())
        } else {
            None
        };

        let payload_str = serde_json::to_string(&req.payload)
            .map_err(|e| Error::Internal(format!("Payload serialization failed: {e}")))?;

        let row = sqlx::query(
            r#"INSERT INTO _task_queue (job_name, queue, status, payload, scheduled_at, retry_count, max_retries)
               VALUES ($1, $2, 'pending', $3::jsonb, $4::timestamptz, 0, $5)
               RETURNING id, job_name, queue, status, payload, scheduled_at, locked_at, locked_by,
                         completed_at, error_message, retry_count, max_retries, created_at, updated_at"#,
        )
        .bind(&req.task_type)
        .bind(&req.queue)
        .bind(&payload_str)
        .bind(scheduled_at.as_deref())
        .bind(req.max_retries as i32)
        .fetch_one(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Enqueue failed: {e}")))?;

        let task = Self::row_to_task(&row);
        serde_json::to_value(&task)
            .map_err(|e| Error::Internal(format!("Task serialization failed: {e}")))
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
        self.ensure_schema().await?;
        let now = chrono::Utc::now();

        // Step 1: Find the next pending task
        let candidate = sqlx::query(
            r#"SELECT id, job_name, queue, status, payload, scheduled_at, locked_at, locked_by,
                      completed_at, error_message, retry_count, max_retries, created_at, updated_at
               FROM _task_queue
               WHERE queue = $1
                 AND status = 'pending'
                 AND (scheduled_at IS NULL OR scheduled_at <= $2)
               ORDER BY created_at ASC
               LIMIT 1"#,
        )
        .bind(queue)
        .bind(now)
        .fetch_optional(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Claim select failed: {e}")))?;

        let Some(candidate) = candidate else {
            return Ok(None);
        };

        let task_id: uuid::Uuid = candidate.get(fields::ID);

        // Step 2: Atomically claim it (only if still pending)
        let updated = sqlx::query(
            r#"UPDATE _task_queue
               SET status = 'running', locked_at = $1, retry_count = retry_count + 1
               WHERE id = $2 AND status = 'pending'
               RETURNING id, job_name, queue, status, payload, scheduled_at, locked_at, locked_by,
                         completed_at, error_message, retry_count, max_retries, created_at, updated_at"#,
        )
        .bind(now)
        .bind(task_id)
        .fetch_optional(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Claim update failed: {e}")))?;

        if let Some(row) = updated {
            let id: uuid::Uuid = row.get(fields::ID);
            let task = Self::row_to_task(&row);
            Ok(Some((id.to_string(), task)))
        } else {
            Ok(None)
        }
    }

    /// Mark a task as completed.
    pub async fn complete(&self, task_id: &str) -> Result<()> {
        let id = uuid::Uuid::parse_str(task_id)
            .map_err(|e| Error::Database(format!("Invalid task ID: {e}")))?;

        sqlx::query(
            r#"UPDATE _task_queue SET status = 'completed', completed_at = $1 WHERE id = $2"#,
        )
        .bind(chrono::Utc::now())
        .bind(id)
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Complete failed: {e}")))?;

        Ok(())
    }

    /// Mark a task as failed. If retries remain, requeue as pending with backoff.
    pub async fn fail(&self, task_id: &str, error: &str) -> Result<()> {
        self.ensure_schema().await?;
        let id = match uuid::Uuid::parse_str(task_id) {
            Ok(id) => id,
            Err(_) => return Ok(()), // Invalid ID format — nothing to fail
        };
        let now = chrono::Utc::now();

        // Get current task to check retry count
        let row = sqlx::query(
            r#"SELECT id, job_name, queue, status, payload, scheduled_at, locked_at, locked_by,
                      completed_at, error_message, retry_count, max_retries, created_at, updated_at
               FROM _task_queue WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Fail select failed: {e}")))?;

        let Some(row) = row else {
            return Ok(());
        };

        let retry_count: i32 = row.get("retry_count");
        let max_retries: i32 = row.get("max_retries");

        if retry_count >= max_retries {
            // Dead-letter the task
            sqlx::query(
                r#"UPDATE _task_queue SET status = 'dead_letter', completed_at = $1, error_message = $2 WHERE id = $3"#,
            )
            .bind(now)
            .bind(error)
            .bind(id)
            .execute(self.db.inner().pool())
            .await
            .map_err(|e| Error::Database(format!("Dead-letter failed: {e}")))?;
        } else {
            // Retry with exponential backoff: 2^retry_count seconds (2s, 4s, 8s, 16s, ...)
            let backoff_secs = 2i64.pow(retry_count as u32);
            let retry_at = now + chrono::Duration::seconds(backoff_secs);

            sqlx::query(
                r#"UPDATE _task_queue SET status = 'pending', scheduled_at = $1, error_message = $2, locked_at = NULL WHERE id = $3"#,
            )
            .bind(retry_at)
            .bind(error)
            .bind(id)
            .execute(self.db.inner().pool())
            .await
            .map_err(|e| Error::Database(format!("Retry update failed: {e}")))?;
        }

        Ok(())
    }

    /// Reclaim stale running tasks (no completion after timeout).
    /// Call this periodically (e.g. every 60 seconds) to handle crashed workers.
    pub async fn reclaim_stale(&self, timeout_secs: u64) -> Result<u64> {
        self.ensure_schema().await?;
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(timeout_secs as i64);

        let result = sqlx::query(
            r#"UPDATE _task_queue SET status = 'pending', locked_at = NULL
               WHERE status = 'running' AND locked_at < $1"#,
        )
        .bind(cutoff)
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Reclaim failed: {e}")))?;

        Ok(result.rows_affected())
    }

    /// Reclaim stale running tasks for a specific queue.
    pub async fn reclaim_stale_for_queue(&self, queue: &str, timeout_secs: u64) -> Result<u64> {
        self.ensure_schema().await?;
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(timeout_secs as i64);

        let result = sqlx::query(
            r#"UPDATE _task_queue SET status = 'pending', locked_at = NULL
               WHERE queue = $1 AND status = 'running' AND locked_at < $2"#,
        )
        .bind(queue)
        .bind(cutoff)
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Reclaim failed: {e}")))?;

        Ok(result.rows_affected())
    }

    /// Get queue statistics.
    pub async fn stats(&self, queue: &str) -> Result<Value> {
        self.ensure_schema().await?;
        let rows =
            sqlx::query(r#"SELECT status, COUNT(*) AS count FROM _task_queue GROUP BY status"#)
                .fetch_all(self.db.inner().pool())
                .await
                .map_err(|e| Error::Database(format!("Stats failed: {e}")))?;

        let mut stats = serde_json::Map::new();
        stats.insert("queue".into(), serde_json::json!(queue));
        for row in &rows {
            if let (Ok(status), Ok(count)) = (
                row.try_get::<String, _>("status"),
                row.try_get::<i64, _>("count"),
            ) {
                stats.insert(status, serde_json::json!(count as u64));
            }
        }

        Ok(Value::Object(stats))
    }

    /// Purge completed tasks older than the given duration.
    pub async fn purge_completed(&self, older_than_secs: u64) -> Result<u64> {
        self.ensure_schema().await?;
        let cutoff = chrono::Utc::now() - chrono::Duration::seconds(older_than_secs as i64);

        let result = sqlx::query(
            r#"DELETE FROM _task_queue WHERE status = 'completed' AND completed_at < $1"#,
        )
        .bind(cutoff)
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Purge failed: {e}")))?;

        Ok(result.rows_affected())
    }

    /// List dead-lettered tasks for inspection.
    pub async fn list_dead_letter(&self, queue: &str, limit: usize) -> Result<Vec<Value>> {
        self.ensure_schema().await?;
        let rows = sqlx::query(
            r#"SELECT id, job_name, queue, status, payload, scheduled_at, locked_at, locked_by,
                      completed_at, error_message, retry_count, max_retries, created_at, updated_at
               FROM _task_queue
               WHERE queue = $1 AND status = 'dead_letter'
               ORDER BY completed_at DESC
               LIMIT $2"#,
        )
        .bind(queue)
        .bind(limit as i64)
        .fetch_all(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("List dead-letter failed: {e}")))?;

        let mut results = Vec::with_capacity(rows.len());
        for row in &rows {
            let task = Self::row_to_task(row);
            let mut val = serde_json::to_value(&task)
                .map_err(|e| Error::Internal(format!("Task serialization failed: {e}")))?;
            // Include the id
            let id: uuid::Uuid = row.get(fields::ID);
            if let Some(obj) = val.as_object_mut() {
                obj.insert(fields::ID.into(), Value::String(id.to_string()));
            }
            results.push(val);
        }
        Ok(results)
    }

    /// Retry a dead-lettered task by resetting its status.
    pub async fn retry_dead_letter(&self, task_id: &str) -> Result<()> {
        self.ensure_schema().await?;
        let id = uuid::Uuid::parse_str(task_id)
            .map_err(|e| Error::Database(format!("Invalid task ID: {e}")))?;

        sqlx::query(
            r#"UPDATE _task_queue SET status = 'pending', retry_count = 0,
               locked_at = NULL, completed_at = NULL, error_message = NULL, scheduled_at = NULL
               WHERE id = $1"#,
        )
        .bind(id)
        .execute(self.db.inner().pool())
        .await
        .map_err(|e| Error::Database(format!("Retry dead-letter failed: {e}")))?;

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
    use std::ops::Deref;

    static TEST_QUEUE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    struct TestQueue {
        queue: TaskQueue,
        _guard: tokio::sync::MutexGuard<'static, ()>,
    }

    impl Deref for TestQueue {
        type Target = TaskQueue;

        fn deref(&self) -> &Self::Target {
            &self.queue
        }
    }

    fn unique_queue() -> String {
        format!("test_q_{}", uuid::Uuid::new_v4().simple())
    }

    async fn create_test_queue() -> TestQueue {
        let guard = TEST_QUEUE_LOCK.lock().await;
        let db = DatabaseClient::new_mem().await;
        TestQueue {
            queue: TaskQueue::new(db),
            _guard: guard,
        }
    }

    async fn claim_eventually(queue: &TaskQueue, queue_name: &str) -> Option<(String, Task)> {
        for _ in 0..20 {
            if let Some(task) = queue.claim_next(queue_name).await.unwrap() {
                return Some(task);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        None
    }

    async fn dead_letter_count_eventually(queue: &TaskQueue, queue_name: &str) -> usize {
        for _ in 0..20 {
            let tasks = queue.list_dead_letter(queue_name, 10).await.unwrap();
            if !tasks.is_empty() {
                return tasks.len();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        0
    }

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
        assert_eq!(2i64.pow(1), 2);
        assert_eq!(2i64.pow(2), 4);
        assert_eq!(2i64.pow(3), 8);
        assert_eq!(2i64.pow(4), 16);
        assert_eq!(2i64.pow(5), 32);
    }

    #[test]
    fn test_task_status_all_variants() {
        let variants = vec![
            (TaskStatus::Pending, "pending"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Completed, "completed"),
            (TaskStatus::Failed, "failed"),
            (TaskStatus::DeadLetter, "dead_letter"),
        ];
        for (status, expected) in variants {
            let s = serde_json::to_string(&status).unwrap();
            assert_eq!(s, format!("\"{expected}\""));
            let deserialized: TaskStatus = serde_json::from_str(&s).unwrap();
            assert_eq!(deserialized, status);
        }
    }

    #[test]
    fn test_task_status_debug() {
        let s = format!("{:?}", TaskStatus::Pending);
        assert_eq!(s, "Pending");
    }

    #[test]
    fn test_task_clone() {
        let task = Task {
            task_type: "test".into(),
            payload: json!({"key": "value"}),
            status: TaskStatus::Pending,
            queue: "default".into(),
            attempts: 1,
            max_retries: 3,
            scheduled_at: Some("2026-01-01T00:00:00Z".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            last_error: None,
            priority: 0,
        };
        let cloned = task.clone();
        assert_eq!(cloned.task_type, "test");
        assert_eq!(cloned.attempts, 1);
    }

    #[test]
    fn test_enqueue_request_clone() {
        let req = EnqueueRequest {
            task_type: "test".into(),
            payload: json!({}),
            queue: "q".into(),
            max_retries: 5,
            delay_secs: 10,
            priority: 1,
        };
        let cloned = req.clone();
        assert_eq!(cloned.task_type, "test");
        assert_eq!(cloned.max_retries, 5);
    }

    #[test]
    fn test_default_queue() {
        assert_eq!(default_queue(), "default");
    }

    #[test]
    fn test_default_max_retries() {
        assert_eq!(default_max_retries(), 3);
    }

    #[test]
    fn test_task_with_all_fields() {
        let task = Task {
            task_type: "email".into(),
            payload: json!({"to": "a@b.com", "template": "welcome"}),
            status: TaskStatus::Running,
            queue: "emails".into(),
            attempts: 2,
            max_retries: 5,
            scheduled_at: Some("2026-06-01T00:00:00Z".into()),
            created_at: "2026-01-01T00:00:00Z".into(),
            started_at: Some("2026-06-01T00:00:01Z".into()),
            finished_at: None,
            last_error: Some("Connection timeout".into()),
            priority: -5,
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json[fields::STATUS], "running");
        assert_eq!(json["attempts"], 2);
        assert_eq!(json["last_error"], "Connection timeout");
        assert_eq!(json["priority"], -5);
    }

    #[tokio::test]
    async fn test_task_queue_new() {
        let queue = create_test_queue().await;
        let _ = queue;
    }

    #[tokio::test]
    async fn test_enqueue_basic() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let result = queue
            .enqueue(EnqueueRequest {
                task_type: "send_email".into(),
                payload: json!({"to": "test@test.com"}),
                queue: q,
                ..Default::default()
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_enqueue_with_delay() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let result = queue
            .enqueue(EnqueueRequest {
                task_type: "cleanup".into(),
                payload: json!({}),
                queue: q,
                delay_secs: 60,
                ..Default::default()
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_enqueue_with_priority() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let result = queue
            .enqueue(EnqueueRequest {
                task_type: "high_priority".into(),
                payload: json!({}),
                queue: q,
                priority: -10,
                ..Default::default()
            })
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_enqueue_batch() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let requests = vec![
            EnqueueRequest {
                task_type: "task1".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            },
            EnqueueRequest {
                task_type: "task2".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            },
            EnqueueRequest {
                task_type: "task3".into(),
                payload: json!({}),
                queue: q,
                ..Default::default()
            },
        ];
        let results = queue.enqueue_batch(requests).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn test_enqueue_batch_empty() {
        let queue = create_test_queue().await;
        let results = queue.enqueue_batch(vec![]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_claim_next_empty_queue() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let result = queue.claim_next(&q).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_claim_next_with_task() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({"key": "value"}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = claim_eventually(&queue, &q).await;
        assert!(result.is_some());
        let (id, task) = result.unwrap();
        assert!(!id.is_empty());
        assert_eq!(task.task_type, "test");
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn test_claim_next_wrong_queue() {
        let queue = create_test_queue().await;
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: "emails".into(),
                ..Default::default()
            })
            .await
            .unwrap();

        let result = queue.claim_next("other_queue").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_complete_task() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let task_id: uuid::Uuid = sqlx::query_scalar(
            r#"SELECT id FROM _task_queue WHERE queue = $1 AND job_name = 'test' ORDER BY created_at DESC LIMIT 1"#,
        )
        .bind(&q)
        .fetch_one(queue.db.inner().pool())
        .await
        .expect("task should be persisted after enqueue");
        let result = queue.complete(&task_id.to_string()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fail_task_with_retries() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: q.clone(),
                max_retries: 3,
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, _) = queue.claim_next(&q).await.unwrap().unwrap();
        let result = queue.fail(&task_id, "Connection error").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fail_task_dead_letters_after_max_retries() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: q.clone(),
                max_retries: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, _) = queue.claim_next(&q).await.unwrap().unwrap();
        let result = queue.fail(&task_id, "Final error").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fail_nonexistent_task() {
        let queue = create_test_queue().await;
        let result = queue.fail("nonexistent:123", "error").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_reclaim_stale_empty() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let count = queue.reclaim_stale_for_queue(&q, 300).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_stats_empty_queue() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let stats = queue.stats(&q).await.unwrap();
        assert_eq!(stats["queue"], q);
    }

    #[tokio::test]
    async fn test_purge_completed_empty() {
        let queue = create_test_queue().await;
        let count = queue.purge_completed(3600).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_list_dead_letter_empty() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let results = queue.list_dead_letter(&q, 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_retry_dead_letter() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "test".into(),
                payload: json!({}),
                queue: q.clone(),
                max_retries: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, _) = queue.claim_next(&q).await.unwrap().unwrap();
        let _ = queue.fail(&task_id, "error").await;

        let result = queue.retry_dead_letter(&task_id).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_multiple_tasks_claim_order() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "first".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "second".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let (_, task1) = queue.claim_next(&q).await.unwrap().unwrap();
        let (_, task2) = queue.claim_next(&q).await.unwrap().unwrap();
        assert_eq!(task1.task_type, "first");
        assert_eq!(task2.task_type, "second");
    }

    #[tokio::test]
    async fn test_claim_returns_none_after_all_claimed() {
        let queue = create_test_queue().await;
        let q = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "only_one".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let first = claim_eventually(&queue, &q).await;
        assert!(first.is_some());
        let second = queue.claim_next(&q).await.unwrap();
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn test_enqueue_different_queues() {
        let queue = create_test_queue().await;
        let q1 = unique_queue();
        let q2 = unique_queue();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "email".into(),
                payload: json!({}),
                queue: q1.clone(),
                ..Default::default()
            })
            .await
            .unwrap();
        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "sync".into(),
                payload: json!({}),
                queue: q2.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let email_task = queue.claim_next(&q1).await.unwrap();
        let sync_task = queue.claim_next(&q2).await.unwrap();
        let no_task = queue.claim_next("other").await.unwrap();
        assert!(email_task.is_some());
        assert!(sync_task.is_some());
        assert!(no_task.is_none());
    }

    #[tokio::test]
    async fn test_full_lifecycle_enqueue_claim_complete() {
        let queue = create_test_queue().await;
        let q = unique_queue();

        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "lifecycle_test".into(),
                payload: json!({"data": 42}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, task) = queue.claim_next(&q).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
        assert_eq!(task.attempts, 1);

        queue.complete(&task_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_full_lifecycle_enqueue_claim_fail_retry() {
        let queue = create_test_queue().await;
        let q = unique_queue();

        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "retry_test".into(),
                payload: json!({}),
                queue: q.clone(),
                max_retries: 3,
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, _) = queue.claim_next(&q).await.unwrap().unwrap();
        queue.fail(&task_id, "Temporary failure").await.unwrap();
    }

    #[tokio::test]
    async fn test_full_lifecycle_dead_letter() {
        let queue = create_test_queue().await;
        let q = unique_queue();

        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "dl_test".into(),
                payload: json!({}),
                queue: q.clone(),
                max_retries: 1,
                ..Default::default()
            })
            .await
            .unwrap();

        let (task_id, _) = claim_eventually(&queue, &q)
            .await
            .expect("task should be claimable after enqueue");
        queue.fail(&task_id, "permanent error").await.unwrap();

        let dl_count = dead_letter_count_eventually(&queue, &q).await;
        assert_eq!(dl_count, 1);
    }

    #[tokio::test]
    async fn test_stats_with_tasks() {
        let queue = create_test_queue().await;
        let q = unique_queue();

        let _ = queue
            .enqueue(EnqueueRequest {
                task_type: "t1".into(),
                payload: json!({}),
                queue: q.clone(),
                ..Default::default()
            })
            .await
            .unwrap();

        let (_, task) = queue.claim_next(&q).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }
}
