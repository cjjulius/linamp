use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use rusqlite::{params, Connection, Row};
use walkdir::WalkDir;

use crate::playlist::{is_supported, Track};

/// Persistent media library backed by a local SQLite database.
pub struct Library {
    conn: Connection,
}

impl Library {
    /// Open (creating if needed) the library DB under the user's local data dir.
    pub fn open() -> Result<Self> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir).ok();
        let conn = Connection::open(dir.join("library.db"))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tracks (
                path          TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                artist        TEXT NOT NULL,
                album         TEXT NOT NULL,
                duration_secs INTEGER,
                sample_rate   INTEGER,
                bitrate       INTEGER,
                channels      INTEGER
            );",
        )?;
        Ok(Self { conn })
    }

    /// Insert or update a single track keyed by its path.
    pub fn upsert(&self, t: &Track) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tracks
                (path,title,artist,album,duration_secs,sample_rate,bitrate,channels)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(path) DO UPDATE SET
                title=?2, artist=?3, album=?4, duration_secs=?5,
                sample_rate=?6, bitrate=?7, channels=?8",
            params![
                t.path.to_string_lossy(),
                t.title,
                t.artist,
                t.album,
                t.duration.map(|d| d.as_secs() as i64),
                t.sample_rate.map(|v| v as i64),
                t.bitrate.map(|v| v as i64),
                t.channels.map(|v| v as i64),
            ],
        )?;
        Ok(())
    }

    /// Recursively scan `dir`, adding every supported file. Returns the count.
    pub fn scan_folder(&self, dir: &Path) -> Result<usize> {
        let files: Vec<PathBuf> = WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| is_supported(p))
            .collect();

        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for p in files {
            let t = Track::from_path(p);
            self.upsert(&t)?;
            n += 1;
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// All tracks, sorted, up to `limit`.
    pub fn all(&self, limit: usize) -> Vec<Track> {
        self.query(
            "SELECT path,title,artist,album,duration_secs,sample_rate,bitrate,channels
             FROM tracks ORDER BY artist, album, title LIMIT ?1",
            params![limit as i64],
        )
    }

    /// Case-insensitive search across title/artist/album, up to `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Track> {
        let like = format!("%{}%", query);
        self.query(
            "SELECT path,title,artist,album,duration_secs,sample_rate,bitrate,channels
             FROM tracks
             WHERE title LIKE ?1 OR artist LIKE ?1 OR album LIKE ?1
             ORDER BY artist, album, title LIMIT ?2",
            params![like, limit as i64],
        )
    }

    fn query(&self, sql: &str, p: impl rusqlite::Params) -> Vec<Track> {
        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(p, row_to_track) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(Result::ok).collect()
    }
}

fn row_to_track(row: &Row) -> rusqlite::Result<Track> {
    let path: String = row.get(0)?;
    let dur: Option<i64> = row.get(4)?;
    let sr: Option<i64> = row.get(5)?;
    let br: Option<i64> = row.get(6)?;
    let ch: Option<i64> = row.get(7)?;
    Ok(Track {
        path: PathBuf::from(path),
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        duration: dur.map(|s| Duration::from_secs(s.max(0) as u64)),
        sample_rate: sr.map(|v| v as u32),
        bitrate: br.map(|v| v as u32),
        channels: ch.map(|v| v as u8),
        loaded: true,
    })
}

fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("LinAmp")
}
