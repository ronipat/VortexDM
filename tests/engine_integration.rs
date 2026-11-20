use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vortexdm::engine::{start_engine, start_engine_with_options};
use vortexdm::models::{
    DownloadItem, DownloadKind, DownloadStatus, EngineCommand, NewDownloadRequest,
    NewTorrentRequest, TorrentSource,
};
use vortexdm::storage::AppStorage;
use vortexdm::torrent::TorrentManager;

#[derive(Clone)]
struct ServerMode {
    reject_head: bool,
    force_200_on_range: bool,
    etag: String,
    chunk_delay_ms: u64,
}

async fn spawn_mock_http_server(data: Arc<Vec<u8>>, mode: ServerMode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let data = data.clone();
            let mode = mode.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let Ok(n) = socket.read(&mut buf).await else {
                    return;
                };
                if n == 0 {
                    return;
                }
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let mut lines = req_str.lines();
                let first_line = lines.next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("GET");
                let _path = parts.next().unwrap_or("/");

                let mut headers: HashMap<String, String> = HashMap::new();
                for line in lines {
                    if let Some((k, v)) = line.split_once(':') {
                        headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                    }
                }

                let total = data.len();

                if method == "HEAD" {
                    if mode.reject_head {
                        let resp = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(resp.as_bytes()).await;
                        return;
                    }
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Length: {}\r\n\
                         Accept-Ranges: bytes\r\n\
                         ETag: {}\r\n\
                         Content-Disposition: attachment; filename=\"payload_test.bin\"\r\n\
                         Content-Type: application/octet-stream\r\n\
                         Connection: close\r\n\r\n",
                        total, mode.etag
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }

                let range_hdr = headers.get("range").cloned();
                let if_range_hdr = headers.get("if-range").cloned();
                let if_range_ok = match if_range_hdr {
                    Some(tok) => tok == mode.etag,
                    None => true,
                };

                if let Some(r) = range_hdr {
                    if !mode.force_200_on_range && if_range_ok {
                        if let Some(spec) = r.strip_prefix("bytes=") {
                            if let Some((s_str, e_str)) = spec.split_once('-') {
                                let start: usize = s_str.parse().unwrap_or(0);
                                let end: usize = e_str
                                    .parse()
                                    .unwrap_or(total.saturating_sub(1))
                                    .min(total.saturating_sub(1));
                                if start <= end && start < total {
                                    let slice = &data[start..=end];
                                    let hdr = format!(
                                        "HTTP/1.1 206 Partial Content\r\n\
                                         Content-Length: {}\r\n\
                                         Content-Range: bytes {}-{}/{}\r\n\
                                         Accept-Ranges: bytes\r\n\
                                         ETag: {}\r\n\
                                         Content-Disposition: attachment; filename=\"payload_test.bin\"\r\n\
                                         Connection: close\r\n\r\n",
                                        slice.len(),
                                        start,
                                        end,
                                        total,
                                        mode.etag
                                    );
                                    if socket.write_all(hdr.as_bytes()).await.is_err() {
                                        return;
                                    }
                                    for chunk in slice.chunks(32 * 1024) {
                                        if mode.chunk_delay_ms > 0 {
                                            tokio::time::sleep(Duration::from_millis(
                                                mode.chunk_delay_ms,
                                            ))
                                            .await;
                                        }
                                        if socket.write_all(chunk).await.is_err() {
                                            return;
                                        }
                                    }
                                    return;
                                }
                            }
                        }
                    }
                }

                let hdr = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Content-Length: {}\r\n\
                     ETag: {}\r\n\
                     Content-Disposition: attachment; filename=\"payload_test.bin\"\r\n\
                     Connection: close\r\n\r\n",
                    total, mode.etag
                );
                if socket.write_all(hdr.as_bytes()).await.is_err() {
                    return;
                }
                for chunk in data.chunks(32 * 1024) {
                    if mode.chunk_delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(mode.chunk_delay_ms)).await;
                    }
                    if socket.write_all(chunk).await.is_err() {
                        return;
                    }
                }
            });
        }
    });

    format!("http://{}/files/test.bin", addr)
}

fn generate_test_payload(size: usize) -> (Arc<Vec<u8>>, String) {
    let mut data = vec![0u8; size];
    let mut x: u64 = 0x5DEECE66D;
    for b in &mut data {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *b = (x >> 32) as u8;
    }
    let digest = hex::encode(Sha256::digest(&data));
    (Arc::new(data), digest)
}

#[tokio::test]
async fn test_multi_segmented_download_with_sha256_verification() {
    let (payload, expected_sha) = generate_test_payload(2 * 1024 * 1024 + 137);
    let url = spawn_mock_http_server(
        payload.clone(),
        ServerMode {
            reject_head: false,
            force_200_on_range: false,
            etag: "\"v1-seg\"".to_string(),
            chunk_delay_ms: 0,
        },
    )
    .await;

    let tmp_dir = std::env::temp_dir().join(format!("vortexdm_test_seg_{}", uuid::Uuid::new_v4()));
    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine(&tokio::runtime::Handle::current(), storage, None);

    engine.send(EngineCommand::Add(NewDownloadRequest {
        url,
        custom_filename: None,
        save_dir: Some(tmp_dir.join("dl")),
        max_connections: Some(8),
        expected_sha256: Some(expected_sha.clone()),
        start_paused: false,
    }));

    let mut completed_item = None;
    for _ in 0..150 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            if item.status == DownloadStatus::Completed {
                completed_item = Some(item.clone());
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Download failed unexpectedly: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let item = completed_item.expect("Download did not complete in time");
    assert_eq!(item.filename, "payload_test.bin");
    assert!(item.supports_ranges);
    assert!(
        item.segments.len() >= 4,
        "Expected segmented download, got {} segments",
        item.segments.len()
    );
    assert_eq!(item.downloaded_bytes, payload.len() as u64);
    assert_eq!(item.computed_sha256.as_deref(), Some(expected_sha.as_str()));
    assert_eq!(item.sha256_verified, Some(true));

    let disk_bytes = std::fs::read(item.target_path()).unwrap();
    assert_eq!(disk_bytes, *payload);

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_head_405_fallback_and_sneaky_200_ok_recovery() {
    let (payload, expected_sha) = generate_test_payload(1_500_000);
    let url = spawn_mock_http_server(
        payload.clone(),
        ServerMode {
            reject_head: true,
            force_200_on_range: true,
            etag: "\"v1-200\"".to_string(),
            chunk_delay_ms: 0,
        },
    )
    .await;

    let tmp_dir = std::env::temp_dir().join(format!("vortexdm_test_200_{}", uuid::Uuid::new_v4()));
    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine(&tokio::runtime::Handle::current(), storage, None);

    engine.send(EngineCommand::Add(NewDownloadRequest {
        url,
        custom_filename: Some("fallback_file.bin".to_string()),
        save_dir: Some(tmp_dir.join("dl")),
        max_connections: Some(6),
        expected_sha256: Some(expected_sha.clone()),
        start_paused: false,
    }));

    let mut completed_item = None;
    for _ in 0..150 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            if item.status == DownloadStatus::Completed {
                completed_item = Some(item.clone());
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Fallback download failed: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let item = completed_item.expect("Fallback download did not complete");
    assert_eq!(item.sha256_verified, Some(true));
    assert_eq!(std::fs::read(item.target_path()).unwrap(), *payload);

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_pause_and_resume_preserves_integrity() {
    let (payload, expected_sha) = generate_test_payload(1_600_000);
    let url = spawn_mock_http_server(
        payload.clone(),
        ServerMode {
            reject_head: false,
            force_200_on_range: false,
            etag: "\"v1-resume\"".to_string(),
            chunk_delay_ms: 15,
        },
    )
    .await;

    let tmp_dir = std::env::temp_dir().join(format!("vortexdm_test_res_{}", uuid::Uuid::new_v4()));
    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine(&tokio::runtime::Handle::current(), storage, None);

    engine.send(EngineCommand::Add(NewDownloadRequest {
        url,
        custom_filename: Some("resumable.bin".to_string()),
        save_dir: Some(tmp_dir.join("dl")),
        max_connections: Some(4),
        expected_sha256: Some(expected_sha.clone()),
        start_paused: false,
    }));

    let mut id = None;
    for _ in 0..60 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            id = Some(item.id);
            if item.downloaded_bytes > 100_000 && item.status == DownloadStatus::Downloading {
                engine.send(EngineCommand::Pause(item.id));
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let dl_id = id.expect("Download item should exist");

    for _ in 0..60 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.iter().find(|d| d.id == dl_id) {
            if item.status == DownloadStatus::Paused {
                assert!(item.downloaded_bytes > 0);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    engine.send(EngineCommand::Resume(dl_id));

    let mut completed_item = None;
    for _ in 0..200 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.iter().find(|d| d.id == dl_id) {
            if item.status == DownloadStatus::Completed {
                completed_item = Some(item.clone());
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Resumed download failed: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let item = completed_item.expect("Resumed download did not complete");
    assert_eq!(item.sha256_verified, Some(true));
    assert_eq!(std::fs::read(item.target_path()).unwrap(), *payload);

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_bittorrent_file_add_metadata_pause_resume_and_remove() {
    let tmp_dir =
        std::env::temp_dir().join(format!("vortexdm_test_torrent_{}", uuid::Uuid::new_v4()));
    let dl_dir = tmp_dir.join("dl");
    std::fs::create_dir_all(&dl_dir).unwrap();

    // Valid minimal BEP-3 bencoded .torrent file (1 piece, 16384 bytes)
    let piece_hash = [0xABu8; 20];
    let mut torrent_bytes = Vec::new();
    torrent_bytes.extend_from_slice(
        b"d4:infod6:lengthi16384e4:name14:sample_iso.bin12:piece lengthi16384e6:pieces20:",
    );
    torrent_bytes.extend_from_slice(&piece_hash);
    torrent_bytes.extend_from_slice(b"ee");

    let torrent_path = tmp_dir.join("sample_iso.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine_with_options(
        &tokio::runtime::Handle::current(),
        storage,
        false, // disable external DHT in isolated test
        false,
        None,
    );

    engine.send(EngineCommand::AddTorrent(NewTorrentRequest {
        source: TorrentSource::FilePath(torrent_path),
        save_dir: Some(dl_dir.clone()),
        start_paused: true,
    }));

    let mut found = None;
    for _ in 0..80 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            if item
                .torrent
                .as_ref()
                .and_then(|t| t.info_hash.as_ref())
                .is_some()
                && item.total_bytes == Some(16384)
            {
                found = Some(item.clone());
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Torrent add failed: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let item = found.expect("Torrent should load metadata cleanly");
    assert_eq!(item.kind, DownloadKind::Torrent);
    assert_eq!(item.filename, "sample_iso.bin");
    assert_eq!(item.total_bytes, Some(16384));
    let tor = item.torrent.as_ref().unwrap();
    assert_eq!(tor.info_hash.as_ref().unwrap().len(), 40);
    assert_eq!(tor.files.len(), 1);
    assert_eq!(tor.files[0].relative_path, "sample_iso.bin");

    engine.send(EngineCommand::Resume(item.id));
    tokio::time::sleep(Duration::from_millis(350)).await;
    engine.send(EngineCommand::Pause(item.id));
    tokio::time::sleep(Duration::from_millis(350)).await;
    engine.send(EngineCommand::Remove {
        id: item.id,
        delete_file: true,
    });
    tokio::time::sleep(Duration::from_millis(350)).await;

    assert!(engine.get_snapshot().downloads.is_empty());

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_ipc_inbox_torrent_auto_starts_and_pause_remove_no_zombie() {
    let tmp_dir =
        std::env::temp_dir().join(format!("vortexdm_test_ipc_{}", uuid::Uuid::new_v4()));
    let meta_dir = tmp_dir.join("meta");
    std::fs::create_dir_all(&meta_dir).unwrap();

    // Minimal valid BEP-3 .torrent file
    let piece_hash = [0xCDu8; 20];
    let mut torrent_bytes = Vec::new();
    torrent_bytes.extend_from_slice(
        b"d4:infod6:lengthi16384e4:name13:auto_open.bin12:piece lengthi16384e6:pieces20:",
    );
    torrent_bytes.extend_from_slice(&piece_hash);
    torrent_bytes.extend_from_slice(b"ee");
    let torrent_path = tmp_dir.join("auto_open.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    let storage = AppStorage::with_dir(meta_dir.clone());
    let engine = start_engine_with_options(
        &tokio::runtime::Handle::current(),
        storage,
        false,
        false,
        None,
    );

    // Simulate a second VortexDM instance launched by Windows Explorer "Open with VortexDM"
    let sender_storage = AppStorage::with_dir(meta_dir);
    let expected_path = torrent_path.display().to_string();
    sender_storage.enqueue_ipc_target(&expected_path).unwrap();

    // Verify running engine picks up the IPC target into the UI confirmation queue & wakes the window
    let mut confirmed_target = None;
    for _ in 0..40 {
        let targets = engine.take_pending_ui_targets();
        if let Some(t) = targets.into_iter().next() {
            confirmed_target = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert_eq!(
        confirmed_target.as_deref(),
        Some(expected_path.as_str()),
        "IPC forwarded .torrent must arrive in the UI confirmation queue"
    );
    assert!(
        engine.take_bring_to_front(),
        "IPC torrent arrival should signal UI to bring window to front"
    );
    assert!(
        engine.get_snapshot().downloads.is_empty(),
        "Torrent should wait for user confirmation before starting"
    );

    // Simulate user clicking "Start Torrent" in the confirmation modal
    engine.send(EngineCommand::AddTorrent(NewTorrentRequest {
        source: TorrentSource::FilePath(torrent_path),
        save_dir: Some(tmp_dir.join("dl")),
        start_paused: false,
    }));

    let mut active_item = None;
    for _ in 0..80 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            if item.filename == "auto_open.bin" && item.status == DownloadStatus::Downloading {
                active_item = Some(item.clone());
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Confirmed torrent failed: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let item = active_item.expect("Confirmed .torrent must start downloading cleanly");

    // Pause active torrent and verify it STAYS paused across multiple 250ms sync ticks
    engine.send(EngineCommand::Pause(item.id));
    tokio::time::sleep(Duration::from_millis(800)).await;
    let snap = engine.get_snapshot();
    let paused = snap
        .downloads
        .iter()
        .find(|d| d.id == item.id)
        .expect("Paused item must still exist");
    assert_eq!(paused.status, DownloadStatus::Paused);
    assert_eq!(paused.speed_bps, 0.0);

    // Remove torrent and verify it STAYS deleted across multiple 250ms sync ticks (no zombie re-adoption)
    engine.send(EngineCommand::Remove {
        id: item.id,
        delete_file: true,
    });
    tokio::time::sleep(Duration::from_millis(850)).await;
    assert!(
        engine.get_snapshot().downloads.is_empty(),
        "Deleted torrent must never reappear via sync_stats"
    );

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_remove_many_bulk_delete_http_and_torrent() {
    let tmp_dir =
        std::env::temp_dir().join(format!("vortexdm_test_bulk_{}", uuid::Uuid::new_v4()));
    let dl_dir = tmp_dir.join("dl");
    std::fs::create_dir_all(&dl_dir).unwrap();

    // Create 2 distinct minimal BEP-3 .torrent files
    let make_tor = |name: &str, fill: u8, file_name: &str| {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            format!(
                "d4:infod6:lengthi16384e4:name{}:{}12:piece lengthi16384e6:pieces20:",
                name.len(),
                name
            )
            .as_bytes(),
        );
        bytes.extend_from_slice(&[fill; 20]);
        bytes.extend_from_slice(b"ee");
        let p = tmp_dir.join(file_name);
        std::fs::write(&p, &bytes).unwrap();
        p
    };
    let tor_a = make_tor("alpha_item.bin", 0x11, "alpha.torrent");
    let tor_b = make_tor("beta_item.bin", 0x22, "beta.torrent");

    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine_with_options(
        &tokio::runtime::Handle::current(),
        storage,
        false,
        false,
        None,
    );

    // Add 1 paused HTTP download + 2 torrents (3 total)
    engine.send(EngineCommand::Add(NewDownloadRequest {
        url: "https://example.invalid/keep_or_drop.zip".to_string(),
        custom_filename: Some("keep_me.zip".to_string()),
        save_dir: Some(dl_dir.clone()),
        max_connections: Some(4),
        expected_sha256: None,
        start_paused: true,
    }));
    engine.send(EngineCommand::AddTorrent(NewTorrentRequest {
        source: TorrentSource::FilePath(tor_a),
        save_dir: Some(dl_dir.clone()),
        start_paused: true,
    }));
    engine.send(EngineCommand::AddTorrent(NewTorrentRequest {
        source: TorrentSource::FilePath(tor_b),
        save_dir: Some(dl_dir.clone()),
        start_paused: true,
    }));

    // Wait until all 3 appear with metadata loaded
    for _ in 0..80 {
        let snap = engine.get_snapshot();
        let ready = snap.downloads.len() == 3
            && snap.downloads.iter().filter(|d| d.is_torrent()).all(|d| {
                d.torrent
                    .as_ref()
                    .and_then(|t| t.info_hash.as_ref())
                    .is_some()
            });
        if ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let snap = engine.get_snapshot();
    assert_eq!(snap.downloads.len(), 3);

    // Bulk-delete both torrents while keeping `keep_me.zip`
    let to_delete: Vec<uuid::Uuid> = snap
        .downloads
        .iter()
        .filter(|d| d.is_torrent())
        .map(|d| d.id)
        .collect();
    assert_eq!(to_delete.len(), 2);

    engine.send(EngineCommand::RemoveMany {
        ids: to_delete,
        delete_file: true,
    });

    tokio::time::sleep(Duration::from_millis(800)).await;
    let after = engine.get_snapshot();
    assert_eq!(after.downloads.len(), 1);
    assert_eq!(after.downloads[0].filename, "keep_me.zip");

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_torrent_refresh_peers_and_sync_stats_no_flicker() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "vortexdm_test_tor_refresh_{}",
        uuid::Uuid::new_v4()
    ));
    let dl_dir = tmp_dir.join("dl");
    let meta_dir = tmp_dir.join("meta");
    std::fs::create_dir_all(&dl_dir).unwrap();
    std::fs::create_dir_all(&meta_dir).unwrap();

    // Minimal valid BEP-3 .torrent file (2 pieces, 32768 bytes)
    let mut torrent_bytes = Vec::new();
    torrent_bytes.extend_from_slice(
        b"d4:infod6:lengthi32768e4:name16:refresh_test.bin12:piece lengthi16384e6:pieces40:",
    );
    torrent_bytes.extend_from_slice(&[0x5Au8; 40]);
    torrent_bytes.extend_from_slice(b"ee");
    let torrent_path = tmp_dir.join("refresh_test.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    let tm = TorrentManager::new(dl_dir.clone(), &meta_dir, false, false);
    let (tid, info_hash, display_name, out_dir) = tm
        .add_torrent(
            &TorrentSource::FilePath(torrent_path),
            Some(dl_dir.clone()),
            false,
            0,
        )
        .await
        .expect("Should add torrent live");

    let sess = tm.get_session().await.expect("Session should be initialized");

    // Wait until torrent transitions from Initializing -> Live
    let mut became_live = false;
    for _ in 0..50 {
        let is_live = sess
            .get(librqbit::api::TorrentIdOrHash::Id(tid))
            .is_some_and(|h| matches!(h.stats().state, librqbit::TorrentStatsState::Live));
        if is_live {
            became_live = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(became_live, "Torrent must become Live before refresh");

    // 1. Live refresh_peers should succeed, return Ok(true), and leave torrent Live (never stuck Paused)
    let refreshed = tm
        .refresh_peers(Some(tid), Some(&info_hash))
        .await
        .expect("refresh_peers should succeed on live torrent");
    assert!(refreshed, "refresh_peers must report true for a live torrent");

    let state_after = sess
        .get(librqbit::api::TorrentIdOrHash::Id(tid))
        .map(|h| h.stats().state);
    assert!(
        matches!(state_after, Some(librqbit::TorrentStatsState::Live)),
        "Torrent must remain Live after refresh_peers, got {:?}",
        state_after
    );

    // 2. Pause torrent in librqbit to simulate the ~5ms transient Paused window during swarm refresh,
    //    and verify `sync_stats` keeps status Downloading & preserves telemetry when `tid` is in `refreshing_tids`.
    tm.pause(Some(tid), Some(&info_hash))
        .await
        .expect("pause should succeed");

    // On an actually paused torrent, refresh_peers must return Ok(false) and NOT unpause it
    let refreshed_paused = tm
        .refresh_peers(Some(tid), Some(&info_hash))
        .await
        .expect("refresh_peers on paused torrent should not error");
    assert!(
        !refreshed_paused,
        "refresh_peers must skip already-paused torrents"
    );

    let mut item = DownloadItem::new_torrent(
        format!("magnet:?xt=urn:btih:{}", info_hash),
        display_name,
        out_dir,
        Some(tid),
        Some(info_hash.clone()),
        false,
    );
    item.status = DownloadStatus::Downloading;
    item.speed_bps = 2_400_000.0;
    item.active_connections = 9;
    item.speed_history.push_back(2_400_000.0);
    if let Some(t) = item.torrent.as_mut() {
        t.peers_live = 9;
        t.peers_seeders = 4;
        t.peers_leechers = 5;
        t.upload_speed_bps = 128_000.0;
    }

    let mut downloads = vec![item];
    let mut prev_bytes = HashMap::new();
    let empty_tids = HashSet::new();
    let empty_hashes = HashSet::new();
    let mut pending_pause = HashSet::new();
    let mut refreshing = HashSet::new();
    refreshing.insert(tid);

    let (dl_total, ul_total, peers_total, _) = TorrentManager::sync_stats(
        &sess,
        &dl_dir,
        &mut downloads,
        &mut prev_bytes,
        &empty_tids,
        &empty_hashes,
        &mut pending_pause,
        &refreshing,
        90,
    );

    assert_eq!(
        downloads[0].status,
        DownloadStatus::Downloading,
        "sync_stats must NOT flicker status to Paused while tid is in refreshing_tids"
    );
    assert_eq!(downloads[0].speed_bps, 2_400_000.0);
    assert_eq!(downloads[0].active_connections, 9);
    assert_eq!(downloads[0].speed_history.len(), 1);
    assert_eq!(dl_total, 2_400_000.0);
    assert_eq!(ul_total, 128_000.0);
    assert_eq!(peers_total, 9);
    let tor = downloads[0].torrent.as_ref().unwrap();
    assert_eq!(tor.peers_live, 9);
    assert_eq!(tor.peers_seeders, 4);
    assert_eq!(tor.peers_leechers, 5);

    // 3. If user explicitly pauses mid-refresh (tid in BOTH pending_pause_tids and refreshing_tids),
    //    user pause MUST win immediately.
    pending_pause.insert(tid);
    TorrentManager::sync_stats(
        &sess,
        &dl_dir,
        &mut downloads,
        &mut prev_bytes,
        &empty_tids,
        &empty_hashes,
        &mut pending_pause,
        &refreshing,
        90,
    );
    assert_eq!(
        downloads[0].status,
        DownloadStatus::Paused,
        "User pause (pending_pause_tids) must take precedence over refreshing_tids"
    );

    tm.shutdown().await;
    let _ = std::fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn test_engine_refresh_torrent_peers_command_and_pause() {
    let tmp_dir = std::env::temp_dir().join(format!(
        "vortexdm_test_cmd_refresh_{}",
        uuid::Uuid::new_v4()
    ));
    let dl_dir = tmp_dir.join("dl");
    std::fs::create_dir_all(&dl_dir).unwrap();

    let mut torrent_bytes = Vec::new();
    torrent_bytes.extend_from_slice(
        b"d4:infod6:lengthi16384e4:name15:cmd_refresh.bin12:piece lengthi16384e6:pieces20:",
    );
    torrent_bytes.extend_from_slice(&[0x77u8; 20]);
    torrent_bytes.extend_from_slice(b"ee");
    let torrent_path = tmp_dir.join("cmd_refresh.torrent");
    std::fs::write(&torrent_path, &torrent_bytes).unwrap();

    let storage = AppStorage::with_dir(tmp_dir.join("meta"));
    let engine = start_engine_with_options(
        &tokio::runtime::Handle::current(),
        storage,
        false,
        false,
        None,
    );

    engine.send(EngineCommand::AddTorrent(NewTorrentRequest {
        source: TorrentSource::FilePath(torrent_path),
        save_dir: Some(dl_dir),
        start_paused: false,
    }));

    let mut item_id = None;
    for _ in 0..80 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.first() {
            if item.status == DownloadStatus::Downloading
                && item
                    .torrent
                    .as_ref()
                    .and_then(|t| t.rqbit_id)
                    .is_some()
            {
                item_id = Some(item.id);
                break;
            }
            if item.status == DownloadStatus::Failed {
                panic!("Torrent failed: {:?}", item.error_message);
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let id = item_id.expect("Torrent should reach Downloading state");

    // Trigger manual swarm refresh command and verify timestamp updates while staying Downloading
    engine.send(EngineCommand::RefreshTorrentPeers(id));

    let mut refreshed_item = None;
    for _ in 0..60 {
        let snap = engine.get_snapshot();
        if let Some(item) = snap.downloads.iter().find(|d| d.id == id) {
            assert_ne!(
                item.status,
                DownloadStatus::Paused,
                "Status must never flicker to Paused during RefreshTorrentPeers"
            );
            if item
                .torrent
                .as_ref()
                .and_then(|t| t.last_swarm_refresh_at)
                .is_some()
            {
                refreshed_item = Some(item.clone());
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let item = refreshed_item.expect("RefreshTorrentPeers should record last_swarm_refresh_at");
    assert_eq!(item.status, DownloadStatus::Downloading);

    // Verify subsequent user Pause still pauses cleanly and stays Paused
    engine.send(EngineCommand::Pause(id));
    tokio::time::sleep(Duration::from_millis(600)).await;
    let snap = engine.get_snapshot();
    let paused = snap.downloads.iter().find(|d| d.id == id).unwrap();
    assert_eq!(paused.status, DownloadStatus::Paused);

    engine.send(EngineCommand::Shutdown);
    let _ = std::fs::remove_dir_all(&tmp_dir);
}
