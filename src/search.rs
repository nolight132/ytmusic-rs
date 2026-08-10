use anyhow::Result;
use serde_json::json;

use crate::client::YtMusic;
use crate::context::Client;
use crate::models::Track;
use crate::nav::Nav as _;
use crate::parse;

pub const SONGS: &str = "EgWKAQIIAQ%3D%3D";
pub const ALBUMS: &str = "EgWKAQIYAQ%3D%3D";
pub const ARTISTS: &str = "EgWKAQIgAQ%3D%3D";
pub const PLAYLISTS: &str = "EgWKAQIoAQ%3D%3D";

impl YtMusic {
    pub async fn search_songs(&self, query: &str) -> Result<Vec<Track>> {
        let response = self
            .execute(
                "search",
                Client::Music,
                json!({ "query": query, "params": SONGS }),
            )
            .await?;
        let mut tracks = Vec::new();
        for shelf in parse::find_renderers(&response, "musicShelfRenderer") {
            for item in shelf.items(&["contents"]) {
                if let Some(track) = parse::list_item_track(item)
                    && track.video_id.is_some()
                {
                    tracks.push(track);
                }
            }
        }
        Ok(tracks)
    }

    pub async fn search_suggestions(&self, input: &str) -> Result<Vec<String>> {
        let response = self
            .execute(
                "music/get_search_suggestions",
                Client::Music,
                json!({ "input": input }),
            )
            .await?;
        Ok(parse::find_renderers(&response, "searchSuggestionRenderer")
            .into_iter()
            .filter_map(|suggestion| suggestion.run_text(&["suggestion"]))
            .collect())
    }
}
