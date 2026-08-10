use serde_json::Value;

use crate::models::{Album, AlbumKind, AlbumRef, ArtistRef, Playlist, Thumbnail, Track};
use crate::nav::{Nav as _, find_all};
use crate::util::{parse_clock, parse_year};

pub const EXPLICIT_BADGE: &str = "MUSIC_EXPLICIT_BADGE";
const GREY_OUT: &str = "MUSIC_ITEM_RENDERER_DISPLAY_POLICY_GREY_OUT";

pub fn thumbnails(node: &Value) -> Vec<Thumbnail> {
    let mut found = Vec::new();
    find_all(node, "thumbnails", &mut found);
    let Some(list) = found.first().and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    let mut thumbs: Vec<Thumbnail> = list
        .iter()
        .filter_map(|thumb| {
            Some(Thumbnail {
                url: thumb.str_at(&["url"])?.to_string(),
                width: thumb.at(&["width"]).and_then(Value::as_u64).unwrap_or(0) as u32,
                height: thumb.at(&["height"]).and_then(Value::as_u64).unwrap_or(0) as u32,
            })
        })
        .collect();
    thumbs.sort_by_key(|thumb| thumb.width);
    thumbs
}

pub fn runs_text(node: &Value) -> Option<String> {
    node.run_text(&[])
}

fn run_browse_id(run: &Value) -> Option<&str> {
    run.str_at(&["navigationEndpoint", "browseEndpoint", "browseId"])
}

pub fn artist_runs(runs: &[Value]) -> Vec<ArtistRef> {
    runs.iter()
        .filter_map(|run| {
            let name = run.str_at(&["text"])?.to_string();
            match run_browse_id(run) {
                Some(id) if id.starts_with("UC") => Some(ArtistRef {
                    name,
                    id: Some(id.to_string()),
                }),
                _ => None,
            }
        })
        .collect()
}

fn album_run(runs: &[Value]) -> Option<AlbumRef> {
    runs.iter().find_map(|run| {
        let id = run_browse_id(run)?;
        id.starts_with("MPR").then(|| AlbumRef {
            name: run.str_at(&["text"]).unwrap_or_default().to_string(),
            id: Some(id.to_string()),
        })
    })
}

fn plain_artist_runs(runs: &[Value]) -> Vec<ArtistRef> {
    let mut artists = Vec::new();
    for run in runs {
        let text = run.str_at(&["text"]).unwrap_or_default();
        if matches!(text, "" | " • " | ", " | " & ") {
            continue;
        }
        if text.chars().all(|c| c.is_ascii_digit() || c == ':') {
            break;
        }
        artists.push(ArtistRef {
            name: text.to_string(),
            id: run_browse_id(run).map(str::to_string),
        });
    }
    artists
}

pub fn explicit(node: &Value) -> bool {
    let mut badges = Vec::new();
    find_all(node, "musicInlineBadgeRenderer", &mut badges);
    badges
        .iter()
        .any(|badge| badge.str_at(&["icon", "iconType"]) == Some(EXPLICIT_BADGE))
}

pub fn list_item_track(item: &Value) -> Option<Track> {
    let renderer = item
        .at(&["musicResponsiveListItemRenderer"])
        .unwrap_or(item);
    let columns: Vec<&Value> = renderer
        .items(&["flexColumns"])
        .iter()
        .filter_map(|column| column.at(&["musicResponsiveListItemFlexColumnRenderer"]))
        .collect();
    let first = columns.first()?;
    let title = first.run_text(&["text"])?;
    let video_id = renderer
        .str_at(&["playlistItemData", "videoId"])
        .or_else(|| {
            first.str_at(&[
                "text",
                "runs",
                "0",
                "navigationEndpoint",
                "watchEndpoint",
                "videoId",
            ])
        })
        .or_else(|| {
            renderer.str_at(&[
                "overlay",
                "musicItemThumbnailOverlayRenderer",
                "content",
                "musicPlayButtonRenderer",
                "playNavigationEndpoint",
                "watchEndpoint",
                "videoId",
            ])
        })
        .map(str::to_string);
    let mut artists = Vec::new();
    let mut album = None;
    for column in columns.iter().skip(1) {
        let runs = column.runs(&["text"]);
        artists.extend(artist_runs(runs));
        if album.is_none() {
            album = album_run(runs);
        }
    }
    if artists.is_empty()
        && let Some(second) = columns.get(1)
    {
        artists = plain_artist_runs(second.runs(&["text"]));
    }
    let duration = renderer
        .items(&["fixedColumns"])
        .first()
        .and_then(|column| column.run_text(&["musicResponsiveListItemFixedColumnRenderer", "text"]))
        .or_else(|| {
            columns.iter().skip(1).find_map(|column| {
                column
                    .runs(&["text"])
                    .iter()
                    .filter_map(|run| run.str_at(&["text"]))
                    .find(|text| {
                        !text.is_empty() && text.chars().all(|c| c.is_ascii_digit() || c == ':')
                    })
                    .map(str::to_string)
            })
        })
        .as_deref()
        .and_then(parse_clock);
    let unavailable = renderer.str_at(&["musicItemRendererDisplayPolicy"]) == Some(GREY_OUT);
    Some(Track {
        available: video_id.is_some() && !unavailable,
        video_id,
        title,
        artists,
        album,
        duration,
        thumbnails: thumbnails(renderer),
        explicit: explicit(renderer),
        set_video_id: renderer
            .str_at(&["playlistItemData", "playlistSetVideoId"])
            .map(str::to_string),
        liked: None,
        views: None,
    })
}

pub fn panel_track(item: &Value) -> Option<Track> {
    let renderer = item
        .at(&["playlistPanelVideoRenderer"])
        .or_else(|| {
            item.at(&[
                "playlistPanelVideoWrapperRenderer",
                "primaryRenderer",
                "playlistPanelVideoRenderer",
            ])
        })
        .unwrap_or(item);
    let video_id = renderer.str_at(&["videoId"])?.to_string();
    let title = renderer.run_text(&["title"])?;
    let byline = renderer.runs(&["longBylineText"]);
    let mut artists = artist_runs(byline);
    if artists.is_empty() {
        artists = plain_artist_runs(byline);
    }
    Some(Track {
        available: true,
        video_id: Some(video_id),
        title,
        artists,
        album: album_run(byline),
        duration: renderer
            .run_text(&["lengthText"])
            .as_deref()
            .and_then(parse_clock),
        thumbnails: thumbnails(renderer),
        explicit: explicit(renderer),
        set_video_id: renderer.str_at(&["playlistSetVideoId"]).map(str::to_string),
        liked: None,
        views: None,
    })
}

pub fn two_row_album(item: &Value) -> Option<Album> {
    let renderer = item.at(&["musicTwoRowItemRenderer"]).unwrap_or(item);
    let browse_id = renderer
        .str_at(&["navigationEndpoint", "browseEndpoint", "browseId"])?
        .to_string();
    if !browse_id.starts_with("MPR") && !browse_id.starts_with("FEmusic_library") {
        return None;
    }
    let title = renderer.run_text(&["title"])?;
    let subtitle_runs = renderer.runs(&["subtitle"]);
    let kind = subtitle_runs
        .iter()
        .filter_map(|run| run.str_at(&["text"]))
        .find_map(album_kind)
        .unwrap_or(AlbumKind::Album);
    let year = subtitle_runs
        .iter()
        .filter_map(|run| run.str_at(&["text"]))
        .find_map(parse_year);
    let mut artists = artist_runs(subtitle_runs);
    if artists.is_empty() {
        artists = subtitle_runs
            .iter()
            .filter_map(|run| run.str_at(&["text"]))
            .filter(|text| {
                !matches!(*text, " • " | ", " | " & ")
                    && album_kind(text).is_none()
                    && parse_year(text).is_none()
            })
            .map(|text| ArtistRef {
                name: text.to_string(),
                id: None,
            })
            .collect();
    }
    let playlist_id = renderer
        .str_at(&[
            "thumbnailOverlay",
            "musicItemThumbnailOverlayRenderer",
            "content",
            "musicPlayButtonRenderer",
            "playNavigationEndpoint",
            "watchPlaylistEndpoint",
            "playlistId",
        ])
        .map(str::to_string);
    Some(Album {
        browse_id,
        playlist_id,
        title,
        artists,
        kind,
        year,
        track_count: None,
        thumbnails: thumbnails(renderer),
    })
}

pub fn album_kind(text: &str) -> Option<AlbumKind> {
    match text {
        "Album" => Some(AlbumKind::Album),
        "Single" => Some(AlbumKind::Single),
        "EP" => Some(AlbumKind::Ep),
        "Compilation" => Some(AlbumKind::Compilation),
        _ => None,
    }
}

pub fn two_row_playlist(item: &Value) -> Option<Playlist> {
    let renderer = item.at(&["musicTwoRowItemRenderer"]).unwrap_or(item);
    let browse_id = renderer.str_at(&["navigationEndpoint", "browseEndpoint", "browseId"])?;
    if !browse_id.starts_with("VL") {
        return None;
    }
    let title = renderer.run_text(&["title"])?;
    let subtitle_runs = renderer.runs(&["subtitle"]);
    let author = subtitle_runs
        .iter()
        .filter_map(|run| run.str_at(&["text"]))
        .find(|text| {
            !matches!(*text, " • " | ", ")
                && !text.starts_with("Playlist")
                && !text.contains("view")
                && !text.contains("song")
                && !text.contains("track")
        })
        .map(str::to_string);
    let track_count = subtitle_runs
        .iter()
        .filter_map(|run| run.str_at(&["text"]))
        .find_map(count_from_text);
    Some(Playlist {
        id: browse_id.trim_start_matches("VL").to_string(),
        title,
        author,
        owned: false,
        track_count,
        thumbnails: thumbnails(renderer),
    })
}

pub fn count_from_text(text: &str) -> Option<u32> {
    let head = text.split_whitespace().next()?;
    let tail = text.split_whitespace().nth(1)?;
    if !tail.starts_with("song") && !tail.starts_with("track") {
        return None;
    }
    head.replace(',', "").parse().ok()
}

pub fn shelf_continuation(node: &Value) -> Option<String> {
    node.str_at(&["continuations", "0", "nextContinuationData", "continuation"])
        .map(str::to_string)
}

pub fn find_renderer<'a>(response: &'a Value, key: &str) -> Option<&'a Value> {
    let mut found = Vec::new();
    find_all(response, key, &mut found);
    found.into_iter().next()
}

pub fn find_renderers<'a>(response: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut found = Vec::new();
    find_all(response, key, &mut found);
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_list_item() {
        let item = json!({
            "musicResponsiveListItemRenderer": {
                "playlistItemData": { "videoId": "abc123", "playlistSetVideoId": "set1" },
                "flexColumns": [
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Song Title", "navigationEndpoint": { "watchEndpoint": { "videoId": "abc123" } } }
                    ] } } },
                    { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                        { "text": "Artist", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCxyz" } } },
                        { "text": " • " },
                        { "text": "Album", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_1" } } }
                    ] } } }
                ],
                "fixedColumns": [
                    { "musicResponsiveListItemFixedColumnRenderer": { "text": { "runs": [ { "text": "3:21" } ] } } }
                ],
                "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                    { "url": "https://img", "width": 60, "height": 60 }
                ] } } },
                "badges": [ { "musicInlineBadgeRenderer": { "icon": { "iconType": "MUSIC_EXPLICIT_BADGE" } } } ]
            }
        });
        let track = list_item_track(&item).unwrap();
        assert_eq!(track.video_id.as_deref(), Some("abc123"));
        assert_eq!(track.title, "Song Title");
        assert_eq!(track.artists.len(), 1);
        assert_eq!(track.artists[0].id.as_deref(), Some("UCxyz"));
        assert_eq!(track.album.as_ref().unwrap().id.as_deref(), Some("MPREb_1"));
        assert_eq!(track.duration, Some(std::time::Duration::from_secs(201)));
        assert!(track.explicit);
        assert!(track.available);
        assert_eq!(track.set_video_id.as_deref(), Some("set1"));
    }

    #[test]
    fn parses_panel_track() {
        let item = json!({
            "playlistPanelVideoRenderer": {
                "videoId": "vid1",
                "title": { "runs": [ { "text": "Radio Song" } ] },
                "lengthText": { "runs": [ { "text": "2:04" } ] },
                "longBylineText": { "runs": [
                    { "text": "Someone", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCa" } } },
                    { "text": " • " },
                    { "text": "An Album", "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_2" } } }
                ] },
                "playlistSetVideoId": "set9"
            }
        });
        let track = panel_track(&item).unwrap();
        assert_eq!(track.video_id.as_deref(), Some("vid1"));
        assert_eq!(track.artists[0].name, "Someone");
        assert_eq!(track.album.as_ref().unwrap().name, "An Album");
        assert_eq!(track.set_video_id.as_deref(), Some("set9"));
    }

    #[test]
    fn parses_two_row_album() {
        let item = json!({
            "musicTwoRowItemRenderer": {
                "navigationEndpoint": { "browseEndpoint": { "browseId": "MPREb_9" } },
                "title": { "runs": [ { "text": "Great Album" } ] },
                "subtitle": { "runs": [
                    { "text": "Album" }, { "text": " • " },
                    { "text": "Artist", "navigationEndpoint": { "browseEndpoint": { "browseId": "UCb" } } },
                    { "text": " • " }, { "text": "2019" }
                ] }
            }
        });
        let album = two_row_album(&item).unwrap();
        assert_eq!(album.browse_id, "MPREb_9");
        assert_eq!(album.kind, AlbumKind::Album);
        assert_eq!(album.year, Some(2019));
        assert_eq!(album.artists[0].name, "Artist");
    }

    #[test]
    fn counts() {
        assert_eq!(count_from_text("34 songs"), Some(34));
        assert_eq!(count_from_text("1 song"), Some(1));
        assert_eq!(count_from_text("1,204 songs"), Some(1204));
        assert_eq!(count_from_text("2016"), None);
    }
}
