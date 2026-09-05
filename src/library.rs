use anyhow::Result;
use serde_json::json;

use crate::client::YtMusic;
use crate::context::Client;
use crate::dedup;
use crate::models::{Album, Playlist, Profile, Track, TrackKind};
use crate::nav::Nav as _;
use crate::parse;

pub const LIKED_SONGS: &str = "LM";
const LIBRARY_ALBUMS: &str = "FEmusic_liked_albums";
const LIBRARY_PLAYLISTS: &str = "FEmusic_liked_playlists";
const LIBRARY_SONGS: &str = "FEmusic_liked_videos";
const MAX_LIBRARY_PAGES: usize = 20;
const RESOLVE_CONCURRENCY: usize = 16;

impl YtMusic {
    pub async fn liked_songs(&self) -> Result<Vec<Track>> {
        let mut response = self.browse_library(LIBRARY_SONGS).await?;
        let mut tracks = Vec::new();
        for _ in 0..MAX_LIBRARY_PAGES {
            tracks.extend(
                parse::find_renderers(&response, "tileRenderer")
                    .into_iter()
                    .filter_map(parse::tv_tile_track),
            );
            let Some(token) = crate::browse::any_continuation(&response) else {
                break;
            };
            response = self
                .execute("browse", Client::Music, json!({ "continuation": token }))
                .await?;
        }
        if tracks.is_empty() {
            let detail = self.playlist(LIKED_SONGS).await?;
            tracks = detail.tracks;
        }
        Ok(dedup::collapse(tracks))
    }

    pub async fn track_duration(&self, video_id: &str) -> Option<std::time::Duration> {
        let response = self
            .execute_music("next", json!({ "videoId": video_id }))
            .await
            .ok()?;
        parse::find_renderers(&response, "playlistPanelVideoRenderer")
            .into_iter()
            .find_map(|renderer| renderer.run_text(&["lengthText"]))
            .as_deref()
            .and_then(crate::util::parse_clock)
    }

    pub async fn resolve_song(&self, track: &Track) -> Result<Option<Track>> {
        if !track.is_video() {
            return Ok(None);
        }
        self.resolve_match(track).await
    }

    pub async fn resolve_match(&self, track: &Track) -> Result<Option<Track>> {
        let query = dedup::search_query(track);
        let candidates = self.search_songs(&query).await?;
        Ok(dedup::best_song_match(track, candidates))
    }

    pub async fn liked_songs_resolved(self: &std::sync::Arc<Self>) -> Result<Vec<Track>> {
        let raw = self.liked_songs().await?;
        let resolved = self.resolve_videos(raw).await;
        Ok(dedup::collapse(resolved))
    }

    pub async fn swap_playable(self: &std::sync::Arc<Self>, tracks: Vec<Track>) -> Vec<Track> {
        use tokio::sync::Semaphore;
        let limit = std::sync::Arc::new(Semaphore::new(RESOLVE_CONCURRENCY));
        let mut tasks = tokio::task::JoinSet::new();
        for (index, mut track) in tracks.into_iter().enumerate() {
            let api = self.clone();
            let limit = limit.clone();
            tasks.spawn(async move {
                let Some(video_id) = track.video_id.clone().filter(|_| track.is_video()) else {
                    return (index, track);
                };
                let song = match api.resolve_cache.get(&video_id).await {
                    Some(cached) => cached,
                    None => {
                        let _permit = limit.acquire().await;
                        let resolved = api.resolve_song(&track).await.ok().flatten();
                        api.resolve_cache.put(video_id, resolved.clone()).await;
                        resolved
                    }
                };
                if let Some(song) = song {
                    track.video_id = song.video_id;
                    if song.duration.is_some() {
                        track.duration = song.duration;
                    }
                    track.kind = TrackKind::Song;
                    if track.album.is_none() {
                        track.album = song.album;
                    }
                }
                (index, track)
            });
        }
        let mut resolved: Vec<(usize, Track)> = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(pair) = result {
                resolved.push(pair);
            }
        }
        self.resolve_cache.flush().await;
        resolved.sort_by_key(|(index, _)| *index);
        resolved.into_iter().map(|(_, track)| track).collect()
    }

    pub async fn resolve_videos(self: &std::sync::Arc<Self>, tracks: Vec<Track>) -> Vec<Track> {
        use tokio::sync::Semaphore;
        let limit = std::sync::Arc::new(Semaphore::new(RESOLVE_CONCURRENCY));
        let mut tasks = tokio::task::JoinSet::new();
        for (index, track) in tracks.into_iter().enumerate() {
            let api = self.clone();
            let limit = limit.clone();
            tasks.spawn(async move {
                let Some(video_id) = track.video_id.clone() else {
                    return (index, track);
                };
                let linkable = track.artists.iter().any(|artist| artist.id.is_some());
                if !track.is_video() && track.album.is_some() && linkable {
                    return (index, track);
                }
                let found = match api.resolve_cache.get(&video_id).await {
                    Some(cached) => cached,
                    None => {
                        let _permit = limit.acquire().await;
                        let resolved = api.resolve_match(&track).await.ok().flatten();
                        api.resolve_cache.put(video_id, resolved.clone()).await;
                        resolved
                    }
                };
                let Some(found) = found else {
                    return (index, track);
                };
                match track.is_video() {
                    true => (index, found),
                    false => {
                        let artists = match track.artists.iter().any(|artist| artist.id.is_some()) {
                            true => track.artists,
                            false => found.artists,
                        };
                        (
                            index,
                            Track {
                                album: track.album.or(found.album),
                                artists,
                                ..track
                            },
                        )
                    }
                }
            });
        }
        let mut resolved: Vec<(usize, Track)> = Vec::new();
        while let Some(result) = tasks.join_next().await {
            if let Ok(pair) = result {
                resolved.push(pair);
            }
        }
        self.resolve_cache.flush().await;
        resolved.sort_by_key(|(index, _)| *index);
        resolved.into_iter().map(|(_, track)| track).collect()
    }

    pub async fn library_albums(&self) -> Result<Vec<Album>> {
        let response = self.browse_library(LIBRARY_ALBUMS).await?;
        Ok(parse::find_renderers(&response, "musicTwoRowItemRenderer")
            .into_iter()
            .filter_map(parse::two_row_album)
            .chain(
                parse::find_renderers(&response, "tileRenderer")
                    .into_iter()
                    .filter_map(parse::tv_tile_album),
            )
            .collect())
    }

    pub async fn library_playlists(&self) -> Result<Vec<Playlist>> {
        let response = self.browse_library(LIBRARY_PLAYLISTS).await?;
        Ok(parse::find_renderers(&response, "musicTwoRowItemRenderer")
            .into_iter()
            .filter_map(parse::two_row_playlist)
            .chain(
                parse::find_renderers(&response, "tileRenderer")
                    .into_iter()
                    .filter_map(parse::tv_tile_playlist),
            )
            .filter(|playlist| playlist.id != LIKED_SONGS)
            .collect())
    }

    pub async fn profile(&self) -> Result<Profile> {
        let response = self
            .execute("account/accounts_list", Client::Tv, json!({}))
            .await?;
        let Some(account) = parse::find_renderer(&response, "accountItem") else {
            log::debug!(
                "profile: accounts_list has no accountItem, response: {}",
                snippet(&response)
            );
            anyhow::bail!("accounts list has no account");
        };
        log::debug!("profile: accountItem: {account}");
        let email = account
            .run_text(&["channelHandle"])
            .or_else(|| account.run_text(&["accountByline"]));
        let name = account
            .run_text(&["accountName"])
            .or_else(|| email.clone())
            .unwrap_or_else(|| {
                log::warn!("profile: account has no name, item: {account}");
                "YouTube Music".to_string()
            });
        Ok(Profile {
            name,
            email,
            thumbnails: parse::thumbnails(account),
        })
    }
}

fn snippet(value: &serde_json::Value) -> String {
    let mut text = value.to_string();
    text.truncate(4000);
    text
}
