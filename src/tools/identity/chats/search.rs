use super::*;

pub(super) async fn mine(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let folder = folder_selector(a)?;
    let query = crate::permalinks::strip_username_prefix(text(a, "query")?, None).to_lowercase();
    ensure!(!query.is_empty(), "Give part of a chat name or username");
    let maximum = limit(a, "limit", 50, 100)? as usize;
    let folders = filters(api).await?;
    let inside = folder.map(|name| find_filter(&folders, name)).transpose()?;
    let found = api
        .invoke("contacts.search", json!({"q":query,"limit":100}))
        .await?;
    let mut local = json!({"users":[],"chats":[]});
    let mut matches = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut page = dialogs(api, 100, None).await?;
    let mut observed = 0;
    let mut scan = json!({"limit":100,"hash":0});
    loop {
        let ds = page["dialogs"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        observed += ds.len();
        for d in ds {
            if let Some(e) = matching_entity(&page, &d["peer"]) {
                let hay = format!(
                    "{} {}",
                    display_name(e),
                    e["username"].as_str().unwrap_or_default()
                )
                .to_lowercase();
                if query.split_whitespace().all(|w| {
                    hay.split(|c: char| !c.is_alphanumeric())
                        .any(|part| part.starts_with(w))
                }) && inside.is_none_or(|f| in_folder(f, e, d, &d["peer"]))
                {
                    matches.push(d["peer"].clone());
                    let table = if d["peer"]["_"] == "peerUser" {
                        "users"
                    } else {
                        "chats"
                    };
                    if let Some(xs) = local[table].as_array_mut() {
                        xs.push(e.clone());
                    }
                }
            }
        }
        if ds.is_empty()
            || (ds.len() < 100
                && (page["_"] != "messages.dialogsSlice"
                    || observed >= page["count"].as_u64().unwrap_or(0) as usize))
        {
            break;
        }
        advance_dialogs(&mut scan, &page, ds)?;
        ensure!(
            seen.insert((
                scan["offset_peer"].to_string(),
                scan["offset_id"].to_string()
            )),
            "Telegram repeated the dialog cursor"
        );
        page = api.invoke("messages.getDialogs", scan.clone()).await?;
    }
    let contacts = api
        .invoke("contacts.getContacts", json!({"hash":0}))
        .await?;
    let mut peers = found
        .get("my_results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    peers.extend(matches);
    for contact in contacts
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let hay = format!(
            "{} {}",
            display_name(contact),
            contact["username"].as_str().unwrap_or("")
        )
        .to_lowercase();
        if query.split_whitespace().all(|word| {
            hay.split(|c: char| !c.is_alphanumeric())
                .any(|part| part.starts_with(word))
        }) {
            peers.push(json!({"_":"peerUser","user_id":req(contact,"id")?}));
        }
    }
    let mut unique = std::collections::HashSet::new();
    peers.retain(|p| unique.insert(peer_key(p)));
    // Limit remote work independently of the result count: many matching
    // contacts may have no dialog, or may be excluded by the selected folder.
    let candidate_limit_reached = peers.len() > 1000;
    peers.truncate(1000);
    let mut candidates = Vec::new();
    for p in &peers {
        if let Some(e) = matching_entity(&found, p)
            .or_else(|| matching_entity(&local, p))
            .or_else(|| matching_entity(&contacts, p))
        {
            candidates.push((p, e, input_from_entity(e)?));
        }
    }
    let mut results = Vec::new();
    'batches: for batch in candidates.chunks(100) {
        let one = api
            .invoke(
                "messages.getPeerDialogs",
                json!({"peers":batch.iter().map(|(_,_,input)|json!({"_":"inputDialogPeer","peer":input})).collect::<Vec<_>>()}),
            )
            .await?;
        for (p, e, input) in batch {
            let Some(d) = one
                .get("dialogs")
                .and_then(Value::as_array)
                .and_then(|dialogs| {
                    dialogs
                        .iter()
                        .find(|dialog| peer_key(&dialog["peer"]) == peer_key(p))
                })
            else {
                continue;
            };
            if inside.is_some_and(|f| !in_folder(f, e, d, input)) {
                continue;
            }
            let mut row = summary(e, d, p)?;
            row["folders"] = folders
                .iter()
                .filter(|f| in_folder(f, e, d, input))
                .map(|f| Value::String(filter_title(f).into()))
                .collect();
            results.push(row);
            if results.len() >= maximum {
                break 'batches;
            }
        }
    }
    Ok(
        json!({"results":results,"metadata":{"candidate_limit_reached":candidate_limit_reached},"untrusted_content":true}),
    )
}

pub(super) async fn lookup(api: &dyn Telegram, a: &Value) -> Result<Value> {
    let query = req(a, "query")?;
    let normalized = crate::tools::identity::normalize_lookup_query(query)?;
    let mut p = api.resolve_peer(&normalized).await?;
    let detail = full(api, &p).await?;
    if kind(&p) == "inputPeerSelf" {
        let me = detail
            .get("users")
            .and_then(Value::as_array)
            .and_then(|xs| xs.first())
            .context("Telegram returned no account identity")?;
        p = json!({"_":"peerUser","user_id":req(me,"id")?});
    }
    let e = matching_entity(&detail, &p).context("Telegram returned no matching peer details")?;
    let mut usernames = e
        .get("usernames")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|u| {
            u.get("username")
                .and_then(Value::as_str)
                .map(|name| json!({"username":name,"active":u["active"]==true}))
        })
        .collect::<Vec<_>>();
    if usernames.is_empty()
        && let Some(name) = e
            .get("username")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    {
        usernames.push(json!({"username":name,"active":true}));
    }
    let link = usernames
        .iter()
        .find(|u| u["active"] == true)
        .and_then(|u| u["username"].as_str())
        .map(|name| crate::permalinks::public_link(name, None))
        .transpose()?;
    Ok(
        json!({"results":[{"id":marked(&p)?,"bare_id":entity_id(&p)?,"type":chat_kind(e),"name":display_name(e),"usernames":usernames,"link":link}],"metadata":{"query":query},"untrusted_content":true}),
    )
}

pub(super) async fn public(api: &dyn Telegram, req: Value) -> Result<Value> {
    let limit = req["limit"].as_u64().unwrap_or(20) as usize;
    let r = api.invoke("contacts.search", req).await?;
    let joined = r["my_results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| marked(p).ok())
        .collect::<std::collections::HashSet<_>>();
    let mut rows = Vec::new();
    for e in ["chats", "users"]
        .iter()
        .flat_map(|k| r[k].as_array().into_iter().flatten())
    {
        let key = match kind(e) {
            "channel" | "channelForbidden" => "channel_id",
            "chat" | "chatForbidden" => "chat_id",
            "user" => "user_id",
            _ => continue,
        };
        let p = json!({"_":match key{"channel_id"=>"peerChannel","chat_id"=>"peerChat",_=>"peerUser"},key:e["id"]});
        let mut row = summary(e, &Value::Null, &p)?;
        row["joined"] = json!(joined.contains(&marked(&p)?));
        rows.push(row);
    }
    let more = rows.len() >= limit;
    Ok(json!({"results":rows,"has_more":more,"metadata":{"returned":rows.len(),"has_more":more}}))
}
