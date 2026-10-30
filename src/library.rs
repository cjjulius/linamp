use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, params_from_iter, Connection, Row};
use walkdir::WalkDir;

use crate::playlist::{is_supported, Track};

/// The eight base columns, in the order `row_to_track` expects.
const COLS: &str = "path,title,artist,album,duration_secs,sample_rate,bitrate,channels";

/// A nav view over the library (the left-hand tree in the window).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibView {
    /// All audio, browsed by artist/album.
    Audio,
    MostPlayed,
    RecentlyAdded,
    RecentlyPlayed,
    NeverPlayed,
    TopRated,
    /// The user's bookmarks. Served from the bookmark store (bookmarks.json),
    /// not the library DB, so it works even for files that were never scanned.
    Bookmarks,
    /// The conversion queue (not backed by the DB): shows active and finished file
    /// conversions. Only appears in the nav while the queue is non-empty.
    Converting,
}

/// Progress messages from a background scan.
pub enum ScanMsg {
    Progress { done: usize, total: usize },
    Done(usize),
    Error(String),
}

/// Persistent media library backed by a local SQLite database.
pub struct Library {
    conn: Connection,
}

impl Library {
    /// Open (creating if needed) the library DB under the user's local data dir.
    pub fn open() -> Result<Self> {
        let dir = data_dir();
        std::fs::create_dir_all(&dir).ok();
        Self::init(Connection::open(dir.join("library.db"))?)
    }

    /// Shared setup: base table, pragmas and forward-only migrations. Split out
    /// so tests can run against an in-memory database.
    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=5000;
             CREATE TABLE IF NOT EXISTS tracks (
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
        Self::migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Forward-only schema migrations, gated by `PRAGMA user_version`.
    fn migrate(conn: &Connection) -> Result<()> {
        let ver: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if ver < 1 {
            // v1: per-track library bookkeeping for the smart views & ratings.
            conn.execute_batch(
                "ALTER TABLE tracks ADD COLUMN date_added  INTEGER;
                 ALTER TABLE tracks ADD COLUMN play_count  INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE tracks ADD COLUMN last_played INTEGER;
                 ALTER TABLE tracks ADD COLUMN rating      INTEGER NOT NULL DEFAULT 0;
                 PRAGMA user_version = 1;",
            )?;
        }
        if ver < 2 {
            // v2: indexes so the browse and smart-view queries don't full-scan and
            // sort on a large library (the search LIKE '%..%' can't use one).
            conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_tracks_artist_album_title
                     ON tracks(artist, album, title);
                 CREATE INDEX IF NOT EXISTS idx_tracks_play_count  ON tracks(play_count);
                 CREATE INDEX IF NOT EXISTS idx_tracks_date_added  ON tracks(date_added);
                 CREATE INDEX IF NOT EXISTS idx_tracks_last_played ON tracks(last_played);
                 CREATE INDEX IF NOT EXISTS idx_tracks_rating      ON tracks(rating);
                 PRAGMA user_version = 2;",
            )?;
        }
        Ok(())
    }

    /// Insert or update a single track keyed by its path. `date_added`,
    /// `play_count`, `last_played` and `rating` are preserved across re-scans.
    pub fn upsert(&self, t: &Track) -> Result<()> {
        // prepare_cached reuses the compiled statement across the thousands of
        // upserts a library scan runs, rather than re-parsing the SQL each call.
        self.conn
            .prepare_cached(
                "INSERT INTO tracks
                (path,title,artist,album,duration_secs,sample_rate,bitrate,channels,date_added)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(path) DO UPDATE SET
                title=?2, artist=?3, album=?4, duration_secs=?5,
                sample_rate=?6, bitrate=?7, channels=?8",
            )?
            .execute(params![
                t.path.to_string_lossy(),
                t.title,
                t.artist,
                t.album,
                t.duration.map(|d| d.as_secs() as i64),
                t.sample_rate.map(|v| v as i64),
                t.bitrate.map(|v| v as i64),
                t.channels.map(|v| v as i64),
                now(),
            ],
        )?;
        Ok(())
    }

    /// Update the stored tags/properties for a track already in the library.
    /// Returns `false` (a no-op) when the path isn't present, so editing a
    /// playlist-only file never silently adds it to the library. Play count,
    /// rating and date_added are left untouched.
    pub fn update_tags(&self, t: &Track) -> bool {
        self.conn
            .execute(
                "UPDATE tracks SET title=?2, artist=?3, album=?4,
                    duration_secs=?5, sample_rate=?6, bitrate=?7, channels=?8
                 WHERE path=?1",
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
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    /// Case-insensitive search across title/artist/album, up to `limit`.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Track> {
        // Escape LIKE wildcards so a literal '%' or '_' in the search text matches
        // itself instead of acting as a pattern (backslash is the escape char).
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let like = format!("%{escaped}%");
        self.query(
            &format!(
                "SELECT {COLS} FROM tracks
                 WHERE title LIKE ?1 ESCAPE '\\' OR artist LIKE ?1 ESCAPE '\\' OR album LIKE ?1 ESCAPE '\\'
                 ORDER BY artist, album, title LIMIT ?2"
            ),
            params![like, limit as i64],
        )
    }

    /// Distinct artists with their album counts, ordered case-insensitively.
    pub fn artists(&self) -> Vec<(String, i64)> {
        self.pairs(
            "SELECT artist, COUNT(DISTINCT album) FROM tracks
             GROUP BY artist ORDER BY artist COLLATE NOCASE",
            params![],
        )
    }

    /// Distinct albums (with track counts) for the given artists; all artists
    /// when the slice is empty.
    pub fn albums(&self, artists: &[String]) -> Vec<(String, i64)> {
        if artists.is_empty() {
            return self.pairs(
                "SELECT album, COUNT(*) FROM tracks
                 GROUP BY album ORDER BY album COLLATE NOCASE",
                params![],
            );
        }
        let sql = format!(
            "SELECT album, COUNT(*) FROM tracks WHERE artist IN ({})
             GROUP BY album ORDER BY album COLLATE NOCASE",
            placeholders(artists.len())
        );
        self.pairs(&sql, params_from_iter(artists.iter()))
    }

    /// Tracks matching the selected artists and albums (empty slice = no filter
    /// on that field).
    pub fn tracks_for(&self, artists: &[String], albums: &[String]) -> Vec<Track> {
        let mut sql = format!("SELECT {COLS} FROM tracks");
        let mut clauses = Vec::new();
        if !artists.is_empty() {
            clauses.push(format!("artist IN ({})", placeholders(artists.len())));
        }
        if !albums.is_empty() {
            clauses.push(format!("album IN ({})", placeholders(albums.len())));
        }
        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY artist, album, title");
        let binds: Vec<&String> = artists.iter().chain(albums.iter()).collect();
        self.query(&sql, params_from_iter(binds))
    }

    /// Tracks for a smart view, up to `limit`.
    pub fn view(&self, view: LibView, limit: usize) -> Vec<Track> {
        let (filter, order) = match view {
            LibView::Audio => ("", "artist, album, title"),
            LibView::MostPlayed => ("WHERE play_count > 0", "play_count DESC, last_played DESC"),
            LibView::RecentlyAdded => ("WHERE date_added IS NOT NULL", "date_added DESC"),
            LibView::RecentlyPlayed => {
                ("WHERE last_played IS NOT NULL", "last_played DESC")
            }
            LibView::NeverPlayed => ("WHERE play_count = 0", "artist, album, title"),
            LibView::TopRated => ("WHERE rating > 0", "rating DESC, artist, album, title"),
            // Bookmarks and the conversion queue aren't DB queries; the UI builds
            // those views from the bookmark store / the live queue.
            LibView::Bookmarks | LibView::Converting => return Vec::new(),
        };
        self.query(
            &format!("SELECT {COLS} FROM tracks {filter} ORDER BY {order} LIMIT ?1"),
            params![limit as i64],
        )
    }

    /// Bump the play count and stamp `last_played` for a track (no-op if the
    /// path isn't in the library).
    pub fn increment_play(&self, path: &Path) {
        let _ = self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?2 WHERE path = ?1",
            params![path.to_string_lossy(), now()],
        );
    }

    /// Remove tracks from the library by path. Only drops the DB rows; the files
    /// on disk are left untouched.
    pub fn remove_paths(&self, paths: &[PathBuf]) {
        for p in paths {
            let _ = self.conn.execute(
                "DELETE FROM tracks WHERE path = ?1",
                params![p.to_string_lossy()],
            );
        }
    }

    /// Set a track's 0–5 star rating (0 clears it).
    pub fn set_rating(&self, path: &Path, rating: i64) {
        let _ = self.conn.execute(
            "UPDATE tracks SET rating = ?2 WHERE path = ?1",
            params![path.to_string_lossy(), rating.clamp(0, 5)],
        );
    }

    /// A single track by its exact path, if the library has it indexed. Used to
    /// give bookmarked tracks their full metadata in the Bookmarks view.
    pub fn track_for_path(&self, path: &Path) -> Option<Track> {
        self.query(
            &format!("SELECT {COLS} FROM tracks WHERE path = ?1 LIMIT 1"),
            params![path.to_string_lossy()],
        )
        .into_iter()
        .next()
    }

    /// Current rating for a track (0 when absent).
    pub fn rating(&self, path: &Path) -> i64 {
        self.conn
            .query_row(
                "SELECT rating FROM tracks WHERE path = ?1",
                params![path.to_string_lossy()],
                |r| r.get(0),
            )
            .unwrap_or(0)
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

    fn pairs(&self, sql: &str, p: impl rusqlite::Params) -> Vec<(String, i64)> {
        let mut stmt = match self.conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match stmt.query_map(p, |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            Ok(rows) => rows,
            Err(_) => return Vec::new(),
        };
        rows.filter_map(Result::ok).collect()
    }
}

/// Scan `dir` on a background thread (its own DB connection), reporting
/// progress over the returned channel. WAL mode lets it write while the app's
/// connection keeps reading.
pub fn scan_folder_bg(dir: PathBuf) -> mpsc::Receiver<ScanMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let lib = match Library::open() {
            Ok(l) => l,
            Err(e) => {
                let _ = tx.send(ScanMsg::Error(e.to_string()));
                return;
            }
        };
        let files = supported_files(&dir);
        let total = files.len();
        // Work in batches, reading each batch's tags OUTSIDE any transaction (the
        // slow, blocking part) and then holding the WAL writer lock only for the
        // inserts. Holding it across the tag reads would starve the app
        // connection's play/rating writes (which then stall or get silently
        // dropped) for the whole batch. Committed rows become visible as it goes.
        const BATCH: usize = 200;
        let mut done = 0usize;
        for chunk in files.chunks(BATCH) {
            let mut tracks = Vec::with_capacity(chunk.len());
            for p in chunk {
                tracks.push(Track::from_path(p.clone()));
                done += 1;
                if done % 25 == 0 {
                    let _ = tx.send(ScanMsg::Progress { done, total });
                }
            }
            // Writer lock held only for the inserts (microseconds each).
            match lib.conn.unchecked_transaction() {
                Ok(txn) => {
                    for t in &tracks {
                        let _ = lib.upsert(t);
                    }
                    let _ = txn.commit();
                }
                Err(_) => {
                    for t in &tracks {
                        let _ = lib.upsert(t);
                    }
                }
            }
        }
        let _ = tx.send(ScanMsg::Done(total));
    });
    rx
}

/// Every supported audio file under `dir`.
fn supported_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .filter(|p| is_supported(p))
        .collect()
}

/// `?,?,…` for an `IN (…)` clause with `n` binds.
fn placeholders(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    impl Library {
        fn open_in_memory() -> Library {
            Library::init(Connection::open_in_memory().unwrap()).unwrap()
        }
    }

    fn track(path: &str, artist: &str, album: &str, title: &str) -> Track {
        Track {
            path: PathBuf::from(path),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            duration: Some(Duration::from_secs(180)),
            sample_rate: Some(44100),
            bitrate: Some(320),
            channels: Some(2),
            loaded: true,
        }
    }

    fn seed(lib: &Library) {
        lib.upsert(&track("/m/a1.mp3", "Alice", "First", "One")).unwrap();
        lib.upsert(&track("/m/a2.mp3", "Alice", "First", "Two")).unwrap();
        lib.upsert(&track("/m/a3.mp3", "Alice", "Second", "Three")).unwrap();
        lib.upsert(&track("/m/b1.mp3", "Bob", "Blue", "Four")).unwrap();
    }

    #[test]
    fn update_tags_only_touches_known_tracks_and_keeps_stats() {
        let lib = Library::open_in_memory();
        lib.upsert(&track("/m/x.mp3", "Old Artist", "Old Album", "Old Title")).unwrap();
        lib.increment_play(&PathBuf::from("/m/x.mp3"));
        lib.set_rating(&PathBuf::from("/m/x.mp3"), 4);

        // A known track is updated in place.
        let edited = track("/m/x.mp3", "New Artist", "New Album", "New Title");
        assert!(lib.update_tags(&edited));
        let hits = lib.search("New Title", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].artist, "New Artist");
        // Play count and rating survive the tag edit.
        assert_eq!(lib.rating(&PathBuf::from("/m/x.mp3")), 4);

        // An unknown path is a no-op and is not inserted.
        let before = lib.count();
        let unknown = track("/m/missing.mp3", "A", "B", "C");
        assert!(!lib.update_tags(&unknown));
        assert_eq!(lib.count(), before);
    }

    #[test]
    fn search_treats_like_wildcards_literally() {
        let lib = Library::open_in_memory();
        lib.upsert(&track("/m/u1.mp3", "A", "B", "track_01")).unwrap();
        lib.upsert(&track("/m/u2.mp3", "A", "B", "trackX01")).unwrap();
        lib.upsert(&track("/m/p1.mp3", "A", "B", "50% off")).unwrap();
        lib.upsert(&track("/m/p2.mp3", "A", "B", "50 cents")).unwrap();

        // '_' must match a literal underscore, not any character.
        let got = lib.search("track_01", 50);
        let titles: Vec<&str> = got.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["track_01"], "underscore matched as a wildcard");

        // '%' must match a literal percent sign, not any sequence.
        let got = lib.search("50%", 50);
        let titles: Vec<&str> = got.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["50% off"], "percent matched as a wildcard");
    }

    #[test]
    fn migration_adds_columns_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        let lib = Library::init(conn).unwrap();
        // user_version advanced to the latest, and the new columns are queryable.
        let ver: i64 = lib
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 2);
        lib.conn
            .query_row("SELECT play_count, rating, date_added, last_played FROM tracks", [], |_| Ok(()))
            .ok();
        // Re-running migrate is a no-op (does not error on duplicate columns).
        Library::migrate(&lib.conn).unwrap();
    }

    #[test]
    fn artists_and_albums_group_with_counts() {
        let lib = Library::open_in_memory();
        seed(&lib);
        let artists = lib.artists();
        assert_eq!(artists, vec![("Alice".into(), 2), ("Bob".into(), 1)]);
        // Alice has two albums; "First" holds two tracks.
        let albums = lib.albums(&["Alice".into()]);
        assert_eq!(albums, vec![("First".into(), 2), ("Second".into(), 1)]);
        // All albums when unfiltered.
        assert_eq!(lib.albums(&[]).len(), 3);
    }

    #[test]
    fn tracks_for_filters_by_artist_and_album() {
        let lib = Library::open_in_memory();
        seed(&lib);
        assert_eq!(lib.tracks_for(&["Alice".into()], &[]).len(), 3);
        assert_eq!(
            lib.tracks_for(&["Alice".into()], &["First".into()]).len(),
            2
        );
        assert_eq!(lib.tracks_for(&[], &["Blue".into()]).len(), 1);
    }

    #[test]
    fn play_count_and_views() {
        let lib = Library::open_in_memory();
        seed(&lib);
        assert_eq!(lib.view(LibView::NeverPlayed, 100).len(), 4);
        lib.increment_play(Path::new("/m/a1.mp3"));
        lib.increment_play(Path::new("/m/a1.mp3"));
        lib.increment_play(Path::new("/m/b1.mp3"));
        assert_eq!(lib.view(LibView::NeverPlayed, 100).len(), 2);
        let most = lib.view(LibView::MostPlayed, 100);
        assert_eq!(most.len(), 2);
        assert_eq!(most[0].path, PathBuf::from("/m/a1.mp3")); // 2 plays first
        assert_eq!(lib.view(LibView::RecentlyPlayed, 100).len(), 2);
    }

    #[test]
    fn rating_roundtrips_and_feeds_top_rated() {
        let lib = Library::open_in_memory();
        seed(&lib);
        assert_eq!(lib.view(LibView::TopRated, 100).len(), 0);
        lib.set_rating(Path::new("/m/a2.mp3"), 5);
        lib.set_rating(Path::new("/m/b1.mp3"), 3);
        assert_eq!(lib.rating(Path::new("/m/a2.mp3")), 5);
        let top = lib.view(LibView::TopRated, 100);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].path, PathBuf::from("/m/a2.mp3")); // 5 stars first
        // Clamped to 0..=5.
        lib.set_rating(Path::new("/m/a2.mp3"), 99);
        assert_eq!(lib.rating(Path::new("/m/a2.mp3")), 5);
    }

    #[test]
    fn track_for_path_finds_indexed_tracks_only() {
        let lib = Library::open_in_memory();
        seed(&lib);
        // An indexed path returns its full metadata; the Bookmarks view relies on
        // this to fill in artist/album for bookmarked tracks.
        let t = lib.track_for_path(Path::new("/m/a1.mp3")).expect("indexed");
        assert_eq!(t.artist, "Alice");
        // A path the library never scanned returns nothing (the UI then falls back
        // to the bookmark's own title).
        assert!(lib.track_for_path(Path::new("/m/not-scanned.mp3")).is_none());
        // Bookmarks is not a DB-backed view.
        assert!(lib.view(LibView::Bookmarks, 100).is_empty());
    }

    #[test]
    fn upsert_preserves_play_count_and_rating() {
        let lib = Library::open_in_memory();
        lib.upsert(&track("/m/a1.mp3", "Alice", "First", "One")).unwrap();
        lib.increment_play(Path::new("/m/a1.mp3"));
        lib.set_rating(Path::new("/m/a1.mp3"), 4);
        // Re-scan (metadata changed) must not reset bookkeeping.
        lib.upsert(&track("/m/a1.mp3", "Alice", "First", "One (remaster)")).unwrap();
        assert_eq!(lib.rating(Path::new("/m/a1.mp3")), 4);
        assert_eq!(lib.view(LibView::MostPlayed, 10).len(), 1);
    }

    #[test]
    fn remove_paths_drops_only_those_rows() {
        let lib = Library::open_in_memory();
        seed(&lib); // four tracks
        lib.remove_paths(&[PathBuf::from("/m/a1.mp3"), PathBuf::from("/m/b1.mp3")]);
        assert_eq!(lib.count(), 2);
        assert!(lib.track_for_path(Path::new("/m/a1.mp3")).is_none());
        assert!(lib.track_for_path(Path::new("/m/a2.mp3")).is_some());
        // Removing an unknown path is a harmless no-op.
        lib.remove_paths(&[PathBuf::from("/m/nope.mp3")]);
        assert_eq!(lib.count(), 2);
    }

    #[test]
    fn recently_added_orders_newest_first() {
        let lib = Library::open_in_memory();
        seed(&lib);
        // Give the rows distinct, known add times (upsert stamps them all with the
        // current second otherwise, leaving same-second order undefined).
        for (p, t) in [("/m/a1.mp3", 100i64), ("/m/a2.mp3", 300), ("/m/a3.mp3", 200), ("/m/b1.mp3", 50)] {
            lib.conn
                .execute("UPDATE tracks SET date_added = ?2 WHERE path = ?1", params![p, t])
                .unwrap();
        }
        let order: Vec<String> = lib
            .view(LibView::RecentlyAdded, 100)
            .iter()
            .map(|t| t.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(order, vec!["/m/a2.mp3", "/m/a3.mp3", "/m/a1.mp3", "/m/b1.mp3"]);
    }

    #[test]
    fn search_matches_artist_and_album_and_respects_limit() {
        let lib = Library::open_in_memory();
        seed(&lib);
        // Matches on the artist field (all three Alice tracks).
        assert_eq!(lib.search("Alice", 100).len(), 3);
        // Matches on the album field.
        let by_album = lib.search("Second", 100);
        assert_eq!(by_album.len(), 1);
        assert_eq!(by_album[0].title, "Three");
        // The LIMIT caps the result count.
        assert_eq!(lib.search("Alice", 2).len(), 2);
    }
}
