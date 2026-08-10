use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistRef {
    pub name: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumRef {
    pub name: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Track {
    pub video_id: Option<String>,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Option<Duration>,
    pub thumbnails: Vec<Thumbnail>,
    pub explicit: bool,
    pub available: bool,
    pub set_video_id: Option<String>,
    pub liked: Option<bool>,
    pub views: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumKind {
    Album,
    Single,
    Ep,
    Compilation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Album {
    pub browse_id: String,
    pub playlist_id: Option<String>,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub kind: AlbumKind,
    pub year: Option<i32>,
    pub track_count: Option<u32>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumDetail {
    pub album: Album,
    pub description: Option<String>,
    pub duration_text: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub author: Option<String>,
    pub owned: bool,
    pub track_count: Option<u32>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub public: bool,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artist {
    pub browse_id: String,
    pub name: String,
    pub description: Option<String>,
    pub subscribers: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
    pub top_tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub singles: Vec<Album>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub email: Option<String>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioFormat {
    pub itag: u32,
    pub url: String,
    pub mime: String,
    pub codec: String,
    pub bitrate: u32,
    pub duration: Option<Duration>,
    pub content_length: Option<u64>,
    pub loudness_db: Option<f32>,
    pub user_agent: &'static str,
}

pub fn best_thumbnail(thumbnails: &[Thumbnail]) -> Option<&Thumbnail> {
    thumbnails.iter().max_by_key(|t| t.width)
}
