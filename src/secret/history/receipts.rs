use super::*;

impl SecretHistory {
    pub(super) fn trim_existing_history(&self) -> Result<()> {
        let mut history = load(&self.history)?;
        let mut changed = false;
        for rows in history.values_mut() {
            let rows = rows
                .as_array_mut()
                .context("Invalid secret-chat history rows")?;
            let before = rows.len();
            retain_history(rows);
            changed |= rows.len() != before;
        }
        if changed {
            save(&self.history, &history)?;
        }
        Ok(())
    }

    pub(in crate::secret) fn commit_delivery(
        &self,
        chat_id: i32,
        seq: i32,
        row: Value,
        reference: Option<&MediaReference>,
    ) -> Result<()> {
        let id = message_id(&row).context("Secret-chat delivery has no message ID")?;
        ensure!(
            row["self_destructs_after_seconds"].as_u64().unwrap_or(0) == 0,
            "Timed secret-chat deliveries cannot be persisted into ordinary consumer snapshots; delivery left pending"
        );
        let _guard = self.guard()?;
        let mut settings = load(&self.settings)?;
        let chat = chat_id.to_string();
        ensure!(
            !forgotten(&settings, &chat),
            "Secret chat was forgotten locally"
        );
        let receipts = settings
            .entry("delivery_receipts")
            .or_insert_with(|| json!({}));
        let key = format!("{chat_id}:{seq}");
        if let Some(saved) = receipts.get(&key) {
            ensure!(
                saved.as_i64() == Some(id),
                "Secret-chat delivery identity changed"
            );
        }
        self.append_locked(chat, row, reference)?;
        receipts
            .as_object_mut()
            .context("Invalid secret-chat delivery receipts")?
            .insert(key, json!(id));
        save(&self.settings, &settings)
    }

    pub(in crate::secret) fn delivery_committed(&self, chat_id: i32, seq: i32) -> Result<bool> {
        let _guard = self.guard()?;
        Ok(load(&self.settings)?
            .get("delivery_receipts")
            .and_then(|receipts| receipts.get(format!("{chat_id}:{seq}")))
            .and_then(Value::as_i64)
            .is_some())
    }
}
