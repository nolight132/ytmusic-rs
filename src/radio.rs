use anyhow::Result;
use serde_json::json;

use crate::client::YtMusic;
use crate::models::Track;
use crate::nav::Nav as _;
use crate::parse;

impl YtMusic {
    pub async fn track_radio(&self, video_id: &str) -> Result<Vec<Track>> {
        let response = self
            .execute_music(
                "next",
                json!({
                    "videoId": video_id,
                    "playlistId": format!("RDAMVM{video_id}"),
                    "enablePersistentPlaylistPanel": true,
                    "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
                }),
            )
            .await?;
        let mut tracks = Vec::new();
        for panel in parse::find_renderers(&response, "playlistPanelRenderer") {
            for item in panel.items(&["contents"]) {
                if let Some(track) = parse::panel_track(item) {
                    tracks.push(track);
                }
            }
        }
        Ok(tracks)
    }
}
