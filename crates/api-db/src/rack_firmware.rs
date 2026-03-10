/*
 * SPDX-FileCopyrightText: Copyright (c) 2026 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
 * SPDX-License-Identifier: Apache-2.0
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::Error::RowNotFound;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use sqlx::{FromRow, PgConnection, Row};

use crate::db_read::DbReader;
use crate::{DatabaseError, DatabaseResult};

// -- RackFirmwareApplyHistory --

#[derive(Debug, Clone)]
pub struct RackFirmwareApplyHistory {
    pub id: i64,
    pub firmware_id: String,
    pub rack_id: String,
    pub firmware_type: String,
    pub applied_at: DateTime<Utc>,
}

impl<'r> FromRow<'r, PgRow> for RackFirmwareApplyHistory {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(RackFirmwareApplyHistory {
            id: row.try_get("id")?,
            firmware_id: row.try_get("firmware_id")?,
            rack_id: row.try_get("rack_id")?,
            firmware_type: row.try_get("firmware_type")?,
            applied_at: row.try_get("applied_at")?,
        })
    }
}

impl RackFirmwareApplyHistory {
    /// Record a firmware apply event
    pub async fn record(
        txn: &mut PgConnection,
        firmware_id: &str,
        rack_id: &str,
        firmware_type: &str,
    ) -> DatabaseResult<Self> {
        let query = "INSERT INTO rack_firmware_apply_history \
            (firmware_id, rack_id, firmware_type) \
            VALUES ($1, $2, $3) RETURNING *";

        sqlx::query_as(query)
            .bind(firmware_id)
            .bind(rack_id)
            .bind(firmware_type)
            .fetch_one(txn)
            .await
            .map_err(|e| DatabaseError::new(query, e))
    }

    /// List apply history, optionally filtered by firmware_id.
    /// Joins against rack_firmware to report whether each firmware_id is still available.
    pub async fn list(
        txn: &mut PgConnection,
        firmware_id: Option<&str>,
    ) -> DatabaseResult<Vec<(Self, bool)>> {
        let mut query = "SELECT h.*, COALESCE(rf.available, false) AS firmware_available \
            FROM rack_firmware_apply_history h \
            LEFT JOIN rack_firmware rf ON rf.id = h.firmware_id"
            .to_string();

        if firmware_id.is_some() {
            query.push_str(" WHERE h.firmware_id = $1");
        }
        query.push_str(" ORDER BY h.applied_at DESC");

        let mut q = sqlx::query_as(&query);
        if let Some(fid) = firmware_id {
            q = q.bind(fid);
        }

        let rows: Vec<RackFirmwareApplyHistoryWithAvailability> = q
            .fetch_all(txn)
            .await
            .map_err(|e| DatabaseError::query(&query, e))?;
        Ok(rows
            .into_iter()
            .map(|r| (r.history, r.firmware_available))
            .collect())
    }
}

/// Internal helper for the joined query result
struct RackFirmwareApplyHistoryWithAvailability {
    history: RackFirmwareApplyHistory,
    firmware_available: bool,
}

impl<'r> FromRow<'r, PgRow> for RackFirmwareApplyHistoryWithAvailability {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(RackFirmwareApplyHistoryWithAvailability {
            history: RackFirmwareApplyHistory::from_row(row)?,
            firmware_available: row.try_get("firmware_available")?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RackFirmware {
    pub id: String,
    pub config: Json<serde_json::Value>,
    pub available: bool,
    pub parsed_components: Option<Json<serde_json::Value>>,
    pub created: DateTime<Utc>,
    pub updated: DateTime<Utc>,
}

impl<'r> FromRow<'r, PgRow> for RackFirmware {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(RackFirmware {
            id: row.try_get("id")?,
            config: row.try_get("config")?,
            available: row.try_get("available")?,
            parsed_components: row.try_get("parsed_components")?,
            created: row.try_get("created")?,
            updated: row.try_get("updated")?,
        })
    }
}
impl From<&RackFirmware> for rpc::forge::RackFirmware {
    fn from(db: &RackFirmware) -> Self {
        let parsed_components = db
            .parsed_components
            .as_ref()
            .map(|p| p.0.to_string())
            .unwrap_or_else(|| "{}".to_string());

        rpc::forge::RackFirmware {
            id: db.id.clone(),
            config_json: db.config.0.to_string(),
            available: db.available,
            created: db.created.format("%Y-%m-%d %H:%M:%S").to_string(),
            updated: db.updated.format("%Y-%m-%d %H:%M:%S").to_string(),
            parsed_components,
        }
    }
}

impl RackFirmware {
    /// Create a new Rack firmware configuration
    pub async fn create(
        txn: &mut PgConnection,
        id: &str,
        config: serde_json::Value,
        parsed_components: Option<serde_json::Value>,
    ) -> DatabaseResult<Self> {
        let query = "INSERT INTO rack_firmware (id, config, parsed_components) VALUES ($1, $2::jsonb, $3::jsonb) RETURNING *";

        sqlx::query_as(query)
            .bind(id)
            .bind(Json(config))
            .bind(parsed_components.map(Json))
            .fetch_one(txn)
            .await
            .map_err(|e| DatabaseError::new(query, e))
    }

    /// Find a Rack firmware configuration by ID
    pub async fn find_by_id(txn: impl DbReader<'_>, id: &str) -> DatabaseResult<Self> {
        let query = "SELECT * FROM rack_firmware WHERE id = $1";
        let ret = sqlx::query_as(query).bind(id).fetch_one(txn).await;
        ret.map_err(|e| match e {
            RowNotFound => DatabaseError::NotFoundError {
                kind: "rack firmware",
                id: format!("{id:?}"),
            },
            _ => DatabaseError::query(query, e),
        })
    }

    /// List all Rack firmware configurations
    pub async fn list_all(
        txn: &mut PgConnection,
        only_available: bool,
    ) -> DatabaseResult<Vec<Self>> {
        let query = if only_available {
            "SELECT * FROM rack_firmware WHERE available = true ORDER BY created DESC"
        } else {
            "SELECT * FROM rack_firmware ORDER BY created DESC"
        };

        sqlx::query_as(query)
            .fetch_all(txn)
            .await
            .map_err(|e| DatabaseError::query(query, e))
    }

    /// Update the configuration
    pub async fn update_config(
        txn: &mut PgConnection,
        id: &str,
        config: serde_json::Value,
    ) -> DatabaseResult<Self> {
        let query = "UPDATE rack_firmware SET config = $2::jsonb, updated = NOW() WHERE id = $1 RETURNING *";

        sqlx::query_as(query)
            .bind(id)
            .bind(Json(config))
            .fetch_one(txn)
            .await
            .map_err(|e| DatabaseError::new(query, e))
    }

    /// Update the available flag
    pub async fn set_available(
        txn: &mut PgConnection,
        id: &str,
        available: bool,
    ) -> DatabaseResult<Self> {
        let query =
            "UPDATE rack_firmware SET available = $2, updated = NOW() WHERE id = $1 RETURNING *";

        sqlx::query_as(query)
            .bind(id)
            .bind(available)
            .fetch_one(txn)
            .await
            .map_err(|e| DatabaseError::new(query, e))
    }

    /// Delete a Rack firmware configuration
    pub async fn delete(txn: &mut PgConnection, id: &str) -> DatabaseResult<()> {
        let query = "DELETE FROM rack_firmware WHERE id = $1 RETURNING id";

        sqlx::query_as::<_, (String,)>(query)
            .bind(id)
            .fetch_one(txn)
            .await
            .map_err(|e| DatabaseError::new(query, e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[crate::sqlx_test]
    async fn test_apply_history_record_and_list(pool: sqlx::PgPool) {
        let mut txn = pool.begin().await.unwrap();

        // Create a firmware config so we can verify the availability join
        RackFirmware::create(&mut txn, "fw-001", json!({"Id": "fw-001"}), None)
            .await
            .unwrap();
        RackFirmware::set_available(&mut txn, "fw-001", true)
            .await
            .unwrap();

        // Record two apply events for the same firmware
        let record1 = RackFirmwareApplyHistory::record(&mut txn, "fw-001", "rack-a", "prod")
            .await
            .unwrap();
        assert_eq!(record1.firmware_id, "fw-001");
        assert_eq!(record1.rack_id, "rack-a");
        assert_eq!(record1.firmware_type, "prod");

        let record2 = RackFirmwareApplyHistory::record(&mut txn, "fw-001", "rack-b", "dev")
            .await
            .unwrap();
        assert_eq!(record2.rack_id, "rack-b");
        assert_eq!(record2.firmware_type, "dev");

        // List all history — should return both, newest first
        let all = RackFirmwareApplyHistory::list(&mut txn, None)
            .await
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0.rack_id, "rack-b");
        assert_eq!(all[1].0.rack_id, "rack-a");
        // firmware is available
        assert!(all[0].1);
        assert!(all[1].1);

        // List filtered by firmware_id
        let filtered = RackFirmwareApplyHistory::list(&mut txn, Some("fw-001"))
            .await
            .unwrap();
        assert_eq!(filtered.len(), 2);

        // Filter by a non-existent firmware_id
        let empty = RackFirmwareApplyHistory::list(&mut txn, Some("fw-999"))
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    #[crate::sqlx_test]
    async fn test_apply_history_firmware_available_reflects_deletion(pool: sqlx::PgPool) {
        let mut txn = pool.begin().await.unwrap();

        // Create firmware and mark available
        RackFirmware::create(&mut txn, "fw-002", json!({"Id": "fw-002"}), None)
            .await
            .unwrap();
        RackFirmware::set_available(&mut txn, "fw-002", true)
            .await
            .unwrap();

        // Record an apply
        RackFirmwareApplyHistory::record(&mut txn, "fw-002", "rack-a", "prod")
            .await
            .unwrap();

        // Verify available = true
        let before = RackFirmwareApplyHistory::list(&mut txn, Some("fw-002"))
            .await
            .unwrap();
        assert_eq!(before.len(), 1);
        assert!(before[0].1);

        // Delete the firmware
        RackFirmware::delete(&mut txn, "fw-002").await.unwrap();

        // History entry still exists but firmware_available is now false
        let after = RackFirmwareApplyHistory::list(&mut txn, Some("fw-002"))
            .await
            .unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].1);
    }

    #[crate::sqlx_test]
    async fn test_apply_history_unavailable_firmware(pool: sqlx::PgPool) {
        let mut txn = pool.begin().await.unwrap();

        // Record history for a firmware_id that was never created
        RackFirmwareApplyHistory::record(&mut txn, "fw-ghost", "rack-a", "prod")
            .await
            .unwrap();

        let history = RackFirmwareApplyHistory::list(&mut txn, None)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].0.firmware_id, "fw-ghost");
        // No matching rack_firmware row — firmware_available should be false
        assert!(!history[0].1);
    }
}
