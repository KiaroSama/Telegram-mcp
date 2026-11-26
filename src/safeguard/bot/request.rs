use super::*;

impl Request {
    pub(in crate::safeguard) async fn wait_detailed(
        &mut self,
        tool: &str,
        args: &Value,
        fingerprint: &str,
        reasons: &[String],
        details: &str,
        allow_always: bool,
    ) -> Result<crate::safeguard::channels::ChannelOutcome> {
        use crate::safeguard::channels::ChannelOutcome;
        let bot = &self.bot;
        let seals = crate::safeguard::sealed::SealedStore::new(Arc::clone(&bot.state));
        let args_display = seals.redact_value(args);
        let delivery_deadline = self
            .deadline
            .min(Instant::now() + Duration::from_secs(DELIVERY_SECONDS));
        let buttons = prompt_buttons(&self.code, allow_always);
        let text = prompt_text(
            &self.code,
            tool,
            &args_display,
            reasons,
            &bot.config.token,
            details,
        );
        seals.seal_prompt(&self.code, &text)?;
        let send_all = async {
            if !matches!(
                tokio::time::timeout_at(delivery_deadline, bot.identify()).await,
                Ok(Ok(()))
            ) {
                return vec![];
            }
            join_all(bot.config.owners.iter().map(|&owner| {
                let body = json!({"chat_id":owner,"text":text,"parse_mode":"HTML","reply_markup":{"inline_keyboard":[buttons]},"link_preview_options":{"is_disabled":true}});
                async move {
                    let result = match tokio::time::timeout_at(delivery_deadline, bot.api("sendMessage", body)).await {
                        Ok(Ok(result)) => result,
                        Ok(Err(error)) if error.downcast_ref::<RejectedRequest>().is_some() => return Ok(None),
                        _ => return Err(()),
                    };
                    let id = result["message_id"].as_i64().filter(|id| *id > 0).ok_or(())?;
                    Ok(Some((owner, id)))
                }
            })).await
        };
        let deliveries = send_all.await;
        if deliveries.iter().any(Result::is_err) {
            crate::safeguard::terminal::remove_pending(&bot.state, fingerprint)?;
            return Ok(ChannelOutcome::AmbiguousDelivery);
        }
        let sent: Vec<_> = deliveries
            .into_iter()
            .filter_map(|delivery| delivery.ok().flatten())
            .collect();
        if !sent.is_empty() {
            let mut pending = bot
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Approval bot request lock is unavailable"))?;
            let request = pending
                .get_mut(&self.code)
                .context("Approval request is no longer pending")?;
            request.messages.extend(sent.iter().copied());
            request.always = allow_always;
        }
        let mut ticket = tokio::time::interval(Duration::from_millis(250));
        let outcome = loop {
            if let Ok(outcome) = self.outcome.try_recv() {
                break outcome;
            }
            if sent.is_empty() {
                break Outcome::Unavailable;
            }
            tokio::select! {
                biased;
                result = &mut self.outcome => break result.unwrap_or(Outcome::Unavailable),
                () = tokio::time::sleep_until(self.deadline) => break Outcome::TimedOut,
                _ = ticket.tick() => {
                    if crate::safeguard::terminal::has_grant(&bot.state, fingerprint, tool, args)? { break Outcome::Approved; }
                }
            }
        };
        let outcome = {
            let mut map = bot
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Approval bot request lock is unavailable"))?;
            if let Some(request) = map.get_mut(&self.code) {
                request.reply.take();
            }
            self.outcome.try_recv().unwrap_or(outcome)
        };
        if outcome == Outcome::TimedOut {
            bot.remove_pending(fingerprint)?;
        }
        let text = match outcome {
            Outcome::Approved => {
                "Approved. The server will recheck the operation before running it."
            }
            Outcome::Denied => "Denied. The operation did not run.",
            Outcome::TimedOut => "Timed out. The operation did not run.",
            Outcome::Unavailable => return Ok(outcome.into()),
        };
        let completed = join_all(sent.into_iter().map(|(owner, message)| bot.api("editMessageText", json!({"chat_id":owner,"message_id":message,"text":text,"reply_markup":{"inline_keyboard":[]}}))));
        let _ = tokio::time::timeout(Duration::from_secs(2), completed).await;
        Ok(outcome.into())
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.bot.pending.lock() {
            pending.remove(&self.code);
        }
    }
}
