use super::*;

impl Mcp {
    /// Starting update streams is an explicit effect of the wait call. Merely
    /// listing tools or checking health never opens Telegram authorizations.
    pub(super) async fn event_api(&self, args: &Value) -> Result<Arc<dyn Telegram>> {
        if let Some(label) = args["account"].as_str() {
            return Ok(Arc::clone(&self.account(label).await?.api));
        }
        let labels = crate::backend::account_labels()?;
        let named_chat = args
            .get("chat_id")
            .filter(|v| !v.is_null())
            .is_some_and(|value| {
                value.as_i64().is_none()
                    && value
                        .as_str()
                        .is_none_or(|text| text.parse::<i64>().is_err())
            });
        if named_chat && labels.len() != 1 {
            bail!("Specify account when resolving a chat username or alias for an event wait");
        }
        let mut only = None;
        for label in &labels {
            only = Some(Arc::clone(&self.account(label).await?.api));
        }
        if labels.len() == 1 {
            return Ok(only.expect("One configured account was opened"));
        }
        Ok(Arc::new(LocalApi(self.clone())))
    }
}
