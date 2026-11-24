//! Read-only migration of Telethon v7/v8 SQLite sessions into native state.
//!
//! Schema source: Telethon sessions/sqlite.py at
//! 8dcbd6bf4405325c6535d2069adafcaa5b01aaa2 (CURRENT_VERSION = 8).
//! Temporary takeout/auth keys and the sent-file cache remain in the legacy
//! database; only permanent authorization, peer capabilities and update cursors
//! have a corresponding durable grammers representation.
use super::PersistentSession;
use anyhow::{Context, Result, bail, ensure};
use grammers_session::{SessionData, types::*};
use rusqlite::{Connection, OpenFlags, Transaction, config::DbConfig, limits::Limit};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::Read,
    net::IpAddr,
    path::Path,
    time::{Duration, Instant},
};

const MAX_DATABASE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PEERS: usize = 100_000;
const MAX_UPDATES: usize = 100_001;

impl PersistentSession {
    /// Import an already-authorized legacy file without opening a Telegram
    /// connection or modifying the source. Stop the legacy server first: a
    /// nonempty SQLite journal/WAL requires its owner to checkpoint and close it.
    pub fn import_telethon_sqlite(&self, path: &Path) -> Result<()> {
        let current = self.data().map_err(anyhow::Error::new)?;
        ensure!(
            !current.dc_options.values().any(|dc| dc.auth_key.is_some()),
            "Refusing to overwrite an existing authorization"
        );
        let mut imported = SessionData {
            home_dc: current.home_dc,
            dc_options: current.dc_options.clone(),
            peer_infos: current.peer_infos.clone(),
            updates_state: current.updates_state.clone(),
        };
        drop(current);
        let source = validate_source(path)?;
        let source_metadata = source
            .metadata()
            .context("Cannot inspect the opened legacy session")?;
        let mut connection = open_source(path)?;
        let transaction = connection
            .transaction()
            .context("Cannot begin a read-only session snapshot")?;
        let version = validate_schema(&transaction)?;
        import_authorization(&transaction, version, &mut imported)?;
        import_peers(&transaction, &mut imported)?;
        import_updates(&transaction, &mut imported)?;
        // Persist the complete validated snapshot first. A failed write leaves
        // the live in-memory session and the old database intact.
        transaction
            .rollback()
            .context("Could not release the legacy session snapshot")?;
        validate_unchanged(path, &source_metadata)?;
        self.commit_import(imported)
    }
}

/// Read only the permanent authorization using the same immutable source,
/// schema, query bounds and source-identity checks as the migration itself.
pub(super) fn authorization_keys(path: &Path) -> Result<Vec<[u8; 256]>> {
    let source = validate_source(path)?;
    let metadata = source
        .metadata()
        .context("Cannot inspect legacy authorization source")?;
    let mut connection = open_source(path)?;
    let transaction = connection
        .transaction()
        .context("Cannot snapshot legacy authorization")?;
    let version = validate_schema(&transaction)?;
    let mut data = SessionData::default();
    import_authorization(&transaction, version, &mut data)?;
    transaction
        .rollback()
        .context("Cannot release legacy authorization snapshot")?;
    validate_unchanged(path, &metadata)?;
    Ok(data
        .dc_options
        .values()
        .filter_map(|dc| dc.auth_key)
        .collect())
}

fn open_source(path: &Path) -> Result<Connection> {
    // immutable=1 prevents SQLite from creating WAL/SHM beside the source,
    // including a checkpointed database whose header still says WAL mode.
    // Nonempty sidecars are refused above; the legacy owner must be stopped.
    let canonical = path
        .canonicalize()
        .context("Cannot resolve the legacy SQLite path")?;
    let mut uri = url::Url::from_file_path(canonical).map_err(|_| {
        anyhow::anyhow!("Legacy SQLite path cannot be represented as a local file URI")
    })?;
    uri.query_pairs_mut()
        .append_pair("mode", "ro")
        .append_pair("immutable", "1");
    let mut connection = Connection::open_with_flags(
        uri.as_str(),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .context("Cannot open the legacy SQLite session read-only")?;
    configure(&mut connection)?;
    Ok(connection)
}
fn validate_unchanged(path: &Path, source_metadata: &std::fs::Metadata) -> Result<()> {
    let current_metadata =
        std::fs::symlink_metadata(path).context("Legacy session path changed during import")?;
    ensure!(
        !current_metadata.file_type().is_symlink()
            && current_metadata.len() == source_metadata.len()
            && current_metadata.modified().ok() == source_metadata.modified().ok(),
        "Legacy session changed during import; stop the legacy server and retry"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            current_metadata.dev() == source_metadata.dev()
                && current_metadata.ino() == source_metadata.ino(),
            "Legacy session identity changed during import"
        );
    }
    Ok(())
}

fn validate_source(path: &Path) -> Result<std::fs::File> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let meta =
            std::fs::symlink_metadata(ancestor).context("Legacy session path is inaccessible")?;
        ensure!(
            !meta.file_type().is_symlink(),
            "Legacy session paths must not contain symlinks"
        );
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            ensure!(
                meta.file_attributes() & 0x400 == 0,
                "Legacy session paths must not contain reparse points"
            );
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000).share_mode(0x1);
    }
    let mut file = options
        .open(path)
        .context("Cannot read the legacy session file")?;
    let meta = file
        .metadata()
        .context("Cannot inspect the legacy session file")?;
    ensure!(
        meta.is_file() && (512..=MAX_DATABASE_BYTES).contains(&meta.len()),
        "Legacy session must be a regular SQLite file between 512 bytes and 128 MiB"
    );
    let mut header = [0u8; 16];
    file.read_exact(&mut header)
        .context("Legacy session header is truncated")?;
    ensure!(
        &header == b"SQLite format 3\0",
        "Legacy session is not a SQLite 3 database"
    );
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        match std::fs::symlink_metadata(Path::new(&sidecar)) {
            Ok(meta) => {
                ensure!(
                    meta.is_file() && !meta.file_type().is_symlink(),
                    "Legacy session sidecar is not a regular file"
                );
                ensure!(
                    meta.len() == 0,
                    "Close and checkpoint the legacy SQLite session before importing its journal or WAL"
                );
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => bail!("Cannot inspect the legacy session sidecars"),
        }
    }
    Ok(file)
}

fn configure(connection: &mut Connection) -> Result<()> {
    connection
        .busy_timeout(Duration::from_millis(250))
        .context("Cannot bound legacy session lock waits")?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .context("Cannot protect the legacy SQLite reader")?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .context("Cannot disable trusted SQLite schemas")?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 1_048_576),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 16_384),
        (Limit::SQLITE_LIMIT_COLUMN, 32),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 32),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_TRIGGER_DEPTH, 0),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        connection
            .set_limit(limit, value)
            .context("Cannot enforce legacy session query bounds")?;
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    connection
        .progress_handler(1000, Some(move || Instant::now() >= deadline))
        .context("Cannot bound legacy SQLite query execution")?;
    connection
        .pragma_update(None, "query_only", true)
        .context("Cannot enforce read-only legacy session queries")?;
    Ok(())
}

fn columns(transaction: &Transaction<'_>, table: &str, expected: &[&str]) -> Result<()> {
    // table is selected only from the fixed internal schema allowlist below.
    let mut statement = transaction
        .prepare(&format!("PRAGMA table_info({table})"))
        .context("Cannot inspect the legacy table schema")?;
    let actual = statement
        .query_map([], |row| row.get::<_, String>(1))
        .context("Cannot read the legacy table columns")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Legacy table columns are invalid")?;
    ensure!(
        actual == expected,
        "Legacy SQLite table has an unsupported column layout"
    );
    Ok(())
}
fn validate_schema(transaction: &Transaction<'_>) -> Result<i64> {
    let mut statement = transaction
        .prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' LIMIT 33")
        .context("Cannot inspect the legacy SQLite schema")?;
    let mut rows = statement
        .query([])
        .context("Cannot read the legacy SQLite schema")?;
    let expected: BTreeSet<String> = [
        "version",
        "sessions",
        "entities",
        "sent_files",
        "update_state",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut tables = BTreeSet::new();
    while let Some(row) = rows.next().context("Legacy SQLite schema is invalid")? {
        let kind: String = row
            .get(0)
            .context("Legacy SQLite schema object type is invalid")?;
        let name: String = row.get(1).context("Legacy SQLite table name is invalid")?;
        let sql: String = row
            .get(2)
            .context("Legacy SQLite table definition is invalid")?;
        ensure!(
            kind == "table"
                && expected.contains(&name)
                && !sql.to_ascii_lowercase().contains("virtual"),
            "Legacy SQLite schema contains an unexpected table, view, index or trigger"
        );
        ensure!(
            tables.insert(name),
            "Legacy SQLite schema contains duplicate tables"
        );
    }
    ensure!(
        tables == expected,
        "Legacy SQLite session is missing required Telethon tables"
    );
    columns(transaction, "version", &["version"])?;
    let versions = transaction
        .prepare("SELECT version FROM version LIMIT 2")
        .context("Cannot read the legacy session version")?
        .query_map([], |row| row.get::<_, i64>(0))
        .context("Cannot read the legacy session version")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("Legacy session version is not an integer")?;
    ensure!(
        versions.len() == 1,
        "Legacy session must contain one schema version"
    );
    let version = versions[0];
    ensure!(
        matches!(version, 7 | 8),
        "Only Telethon SQLite session versions 7 and 8 can be imported"
    );
    let mut session_columns = vec!["dc_id", "server_address", "port", "auth_key", "takeout_id"];
    if version == 8 {
        session_columns.push("tmp_auth_key");
    }
    columns(transaction, "sessions", &session_columns)?;
    columns(
        transaction,
        "entities",
        &["id", "hash", "username", "phone", "name", "date"],
    )?;
    columns(
        transaction,
        "sent_files",
        &["md5_digest", "file_size", "type", "id", "hash"],
    )?;
    columns(
        transaction,
        "update_state",
        &["id", "pts", "qts", "date", "seq"],
    )?;
    Ok(version)
}
fn import_authorization(
    transaction: &Transaction<'_>,
    _version: i64,
    data: &mut SessionData,
) -> Result<()> {
    let mut statement = transaction
        .prepare("SELECT dc_id,server_address,port,auth_key FROM sessions LIMIT 2")
        .context("Cannot read the legacy permanent authorization")?;
    let mut rows = statement
        .query([])
        .context("Cannot read the legacy permanent authorization")?;
    let row = rows
        .next()
        .context("Legacy authorization row is invalid")?
        .context("The legacy SQLite session has no authorization")?;
    let dc: i32 = row.get(0).context("Legacy datacenter id is invalid")?;
    let address: String = row.get(1).context("Legacy datacenter address is invalid")?;
    let port: i64 = row.get(2).context("Legacy datacenter port is invalid")?;
    let key: Vec<u8> = row
        .get(3)
        .context("Legacy permanent authorization key is missing")?;
    ensure!(
        key.len() == 256 && key.iter().any(|byte| *byte != 0),
        "Legacy permanent authorization key must contain 256 nonzero-backed bytes"
    );
    ensure!(
        (1..=65535).contains(&port),
        "Legacy datacenter port is outside 1..65535"
    );
    let ip: IpAddr = address
        .parse()
        .context("Legacy datacenter address must be an IPv4 or IPv6 literal")?;
    ensure!(
        !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast(),
        "Legacy datacenter address must not be unspecified, loopback or multicast"
    );
    let option = data
        .dc_options
        .get_mut(&dc)
        .context("Legacy datacenter is not supported by grammers")?;
    match ip {
        IpAddr::V4(ip) => option.ipv4 = std::net::SocketAddrV4::new(ip, port as u16),
        IpAddr::V6(ip) => option.ipv6 = std::net::SocketAddrV6::new(ip, port as u16, 0, 0),
    }
    option.auth_key = Some(
        key.try_into()
            .map_err(|_| anyhow::anyhow!("Legacy authorization length changed"))?,
    );
    data.home_dc = dc;
    ensure!(
        rows.next()
            .context("Cannot finish reading the legacy authorization")?
            .is_none(),
        "Legacy SQLite session contains multiple authorization rows"
    );
    Ok(())
}
fn import_peers(transaction: &Transaction<'_>, data: &mut SessionData) -> Result<()> {
    let mut statement = transaction
        .prepare("SELECT id,hash FROM entities LIMIT 100001")
        .context("Cannot read legacy peer capabilities")?;
    let mut rows = statement
        .query([])
        .context("Cannot read legacy peer capabilities")?;
    let mut count = 0;
    while let Some(row) = rows
        .next()
        .context("Legacy peer capability row is invalid")?
    {
        count += 1;
        ensure!(
            count <= MAX_PEERS,
            "Legacy peer cache exceeds 100,000 entries"
        );
        let id: i64 = row
            .get(0)
            .context("Legacy peer id is not a signed integer")?;
        let hash: i64 = row
            .get(1)
            .context("Legacy peer access hash is not a signed integer")?;
        let peer = PeerId::from_bot_api_dialog_id(id)
            .context("Legacy peer id is outside Telegram's identifier ranges")?;
        let id = peer.bare_id_unchecked();
        let auth = Some(PeerAuth::from_hash(hash));
        let info = match peer.kind() {
            PeerKind::User => PeerInfo::User {
                id,
                auth,
                bot: None,
                is_self: None,
            },
            PeerKind::Chat => PeerInfo::Chat { id },
            PeerKind::Channel => PeerInfo::Channel {
                id,
                auth,
                kind: None,
            },
        };
        data.peer_infos.insert(peer, info);
    }
    Ok(())
}
fn import_updates(transaction: &Transaction<'_>, data: &mut SessionData) -> Result<()> {
    let mut statement = transaction
        .prepare("SELECT id,pts,qts,date,seq FROM update_state LIMIT 100002")
        .context("Cannot read legacy update cursors")?;
    let mut rows = statement
        .query([])
        .context("Cannot read legacy update cursors")?;
    let mut count = 0;
    while let Some(row) = rows.next().context("Legacy update cursor row is invalid")? {
        count += 1;
        ensure!(
            count <= MAX_UPDATES,
            "Legacy update cursor cache exceeds its entry limit"
        );
        let id: i64 = row.get(0).context("Legacy update cursor id is invalid")?;
        let pts: i32 = row.get(1).context("Legacy update pts is invalid")?;
        let qts: i32 = row.get(2).context("Legacy update qts is invalid")?;
        let date: i32 = row.get(3).context("Legacy update date is invalid")?;
        let seq: i32 = row.get(4).context("Legacy update sequence is invalid")?;
        ensure!(
            pts >= 0 && qts >= 0 && date >= 0 && seq >= 0,
            "Legacy update cursors must be nonnegative"
        );
        if id == 0 {
            data.updates_state.pts = pts;
            data.updates_state.qts = qts;
            data.updates_state.date = date;
            data.updates_state.seq = seq;
        } else {
            ensure!(
                PeerId::channel(id).is_some(),
                "Legacy channel update cursor has an invalid channel id"
            );
            data.updates_state
                .channels
                .retain(|channel| channel.id != id);
            data.updates_state.channels.push(ChannelState { id, pts });
        }
    }
    Ok(())
}
