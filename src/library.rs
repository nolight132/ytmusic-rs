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
const RESOLVE_CONCURRENCY: usize = 6;

impl YtMusic {
    pub async fn liked_songs(&self) -> Result<Vec<Track>> {
        let detail = self.playlist(LIKED_SONGS).await?;
        Ok(dedup::collapse(detail.tracks))
    }

    pub async fn track_duration(&self, video_id: &str) -> Option<std::time::Duration> {
        let response = self
            .execute("next", Client::Music, json!({ "videoId": video_id }))
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
                if !track.is_video() {
                    return (index, track);
                }
                let Some(video_id) = track.video_id.clone() else {
                    return (index, track);
                };
                if let Some(cached) = api.resolve_cache.get(&video_id).await {
                    return (index, cached.unwrap_or(track));
                }
                let _permit = limit.acquire().await;
                let resolved = api.resolve_song(&track).await.ok().flatten();
                api.resolve_cache.put(video_id, resolved.clone()).await;
                (index, resolved.unwrap_or(track))
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
        let response = self
            .execute(
                "browse",
                Client::Music,
                json!({ "browseId": LIBRARY_ALBUMS }),
            )
            .await?;
        Ok(parse::find_renderers(&response, "musicTwoRowItemRenderer")
            .into_iter()
            .filter_map(parse::two_row_album)
            .collect())
    }

    pub async fn library_playlists(&self) -> Result<Vec<Playlist>> {
        let response = self
            .execute(
                "browse",
                Client::Music,
                json!({ "browseId": LIBRARY_PLAYLISTS }),
            )
            .await?;
        Ok(parse::find_renderers(&response, "musicTwoRowItemRenderer")
            .into_iter()
            .filter_map(parse::two_row_playlist)
            .filter(|playlist| playlist.id != LIKED_SONGS)
            .collect())
    }

    pub async fn profile(&self) -> Result<Profile> {
        match self.is_cookie_auth() {
            true => self.profile_from_menu().await,
            false => self.profile_from_accounts().await,
        }
    }

    async fn profile_from_menu(&self) -> Result<Profile> {
        let response = self
            .execute("account/account_menu", Client::Music, json!({}))
            .await?;
        let Some(account) = parse::find_renderer(&response, "activeAccountHeaderRenderer") else {
            log::warn!(
                "profile: account_menu has no active account, response: {}",
                snippet(&response)
            );
            anyhow::bail!("account menu has no active account");
        };
        log::debug!("profile: activeAccountHeaderRenderer: {account}");
        let email = account
            .run_text(&["channelHandle"])
            .or_else(|| account.run_text(&["email"]));
        let name = account
            .run_text(&["accountName"])
            .or_else(|| email.clone())
            .unwrap_or_else(|| "YouTube Music".to_string());
        Ok(Profile {
            name,
            email,
            thumbnails: parse::thumbnails(account),
        })
    }

    async fn profile_from_accounts(&self) -> Result<Profile> {
        let response = self
            .execute("account/accounts_list", Client::Tv, json!({}))
            .await?;
        let Some(account) = parse::find_renderer(&response, "accountItem") else {
            log::warn!(
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
