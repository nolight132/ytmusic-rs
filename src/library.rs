use anyhow::{Context as _, Result};
use serde_json::json;

use crate::client::YtMusic;
use crate::context::Client;
use crate::models::{Album, Playlist, Profile, Track};
use crate::nav::Nav as _;
use crate::parse;

pub const LIKED_SONGS: &str = "LM";
const LIBRARY_ALBUMS: &str = "FEmusic_liked_albums";
const LIBRARY_PLAYLISTS: &str = "FEmusic_liked_playlists";

impl YtMusic {
    pub async fn liked_songs(&self) -> Result<Vec<Track>> {
        let detail = self.playlist(LIKED_SONGS).await?;
        Ok(detail.tracks)
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
        let response = self
            .execute("account/accounts_list", Client::Tv, json!({}))
            .await?;
        let account = parse::find_renderer(&response, "accountItem")
            .context("accounts list has no account")?;
        let name = account
            .run_text(&["accountName"])
            .context("account has no name")?;
        let email = account
            .run_text(&["channelHandle"])
            .or_else(|| account.run_text(&["accountByline"]));
        Ok(Profile {
            name,
            email,
            thumbnails: parse::thumbnails(account),
        })
    }
}
