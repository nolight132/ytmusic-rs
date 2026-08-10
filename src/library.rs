use anyhow::Result;
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
